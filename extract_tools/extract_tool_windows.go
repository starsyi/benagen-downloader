package main

import (
	"archive/zip"
	"crypto/md5"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/app"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/dialog"
	"fyne.io/fyne/v2/widget"
)

// DataExtractor 数据解压器
type DataExtractor struct {
	prefix    string
	outputDir string
	chunks    []string
}

// ExtractorApp GUI 应用
type ExtractorApp struct {
	app              fyne.App
	window           fyne.Window
	prefixSelect     *widget.Select
	outputEntry      *widget.Entry
	verifyCheck      *widget.Check
	logText          *widget.RichText
	logCard          *widget.Card
	logContainer     *fyne.Container
	progressBar      *widget.ProgressBar
	progressLabel    *widget.Label
	startButton      *widget.Button
	toggleLogButton  *widget.Button
	isExtracting     bool
	logVisible       bool
	extractorMutex   sync.Mutex
}

// FindChunks 查找所有 ZIP 分块文件
func (e *DataExtractor) FindChunks() ([]string, error) {
	currentDir, err := os.Getwd()
	if err != nil {
		return nil, err
	}

	matches, err := filepath.Glob(filepath.Join(currentDir, e.prefix+"_*.zip"))
	if err != nil {
		return nil, err
	}

	sort.Strings(matches)
	e.chunks = matches
	return matches, nil
}

// CalculateMD5 计算文件 MD5
func CalculateMD5(filePath string) (string, error) {
	file, err := os.Open(filePath)
	if err != nil {
		return "", err
	}
	defer file.Close()

	hash := md5.New()
	if _, err := io.Copy(hash, file); err != nil {
		return "", err
	}

	return fmt.Sprintf("%x", hash.Sum(nil)), nil
}

// ExtractChunk 解压单个 ZIP 文件
func (e *DataExtractor) ExtractChunk(chunkFile string, progressCallback func(int, int)) error {
	reader, err := zip.OpenReader(chunkFile)
	if err != nil {
		return err
	}
	defer reader.Close()

	totalFiles := len(reader.File)

	for idx, file := range reader.File {
		if progressCallback != nil {
			progressCallback(idx+1, totalFiles)
		}

		if err := extractFile(file, e.outputDir); err != nil {
			fmt.Printf("[WARN] 解压文件 %s 时出错: %v\n", file.Name, err)
		}
	}

	return nil
}

// extractFile 解压单个文件
func extractFile(file *zip.File, outputDir string) error {
	filePath := filepath.Join(outputDir, file.Name)

	// 创建目录
	if file.FileInfo().IsDir() {
		os.MkdirAll(filePath, file.Mode())
		return nil
	}

	// 创建父目录
	if err := os.MkdirAll(filepath.Dir(filePath), 0755); err != nil {
		return err
	}

	// 解压文件
	outFile, err := os.OpenFile(filePath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, file.Mode())
	if err != nil {
		return err
	}
	defer outFile.Close()

	inFile, err := file.Open()
	if err != nil {
		return err
	}
	defer inFile.Close()

	_, err = io.Copy(outFile, inFile)
	return err
}

// VerifyMD5 验证 MD5 完整性
func (e *DataExtractor) VerifyMD5(prefix string) (bool, int, int) {
	md5File := prefix + ".md5"
	content, err := os.ReadFile(md5File)
	if err != nil {
		fmt.Printf("[!] MD5 文件不存在: %s\n", md5File)
		return true, 0, 0
	}

	verifiedCount := 0
	failedCount := 0

	lines := strings.Split(string(content), "\n")
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}

		parts := strings.Fields(line)
		if len(parts) < 2 {
			continue
		}

		expectedHash := parts[0]
		filename := strings.Join(parts[1:], " ")

		filePath := filename
		if !filepath.IsAbs(filePath) {
			filePath = filepath.Join(e.outputDir, filename)
		}

		if _, err := os.Stat(filePath); err != nil {
			fmt.Printf("[!] 文件未找到: %s\n", filename)
			failedCount++
			continue
		}

		actualHash, err := CalculateMD5(filePath)
		if err != nil {
			fmt.Printf("[✗] 计算 MD5 失败: %s\n", filename)
			failedCount++
			continue
		}

		if strings.EqualFold(expectedHash, actualHash) {
			fmt.Printf("[✓] %s\n", filename)
			verifiedCount++
		} else {
			fmt.Printf("[✗] %s (MD5 不匹配!)\n", filename)
			fmt.Printf("    期望: %s\n", expectedHash)
			fmt.Printf("    实际: %s\n", actualHash)
			failedCount++
		}
	}

	return failedCount == 0, verifiedCount, failedCount
}

// AutoDetectPrefixes 自动检测数据包前缀
func AutoDetectPrefixes() ([]string, error) {
	currentDir, err := os.Getwd()
	if err != nil {
		return nil, err
	}

	// 匹配所有符合模式的 ZIP 文件
	matches, err := filepath.Glob(filepath.Join(currentDir, "*_*.zip"))
	if err != nil {
		return nil, err
	}

	prefixes := make(map[string]bool)
	for _, match := range matches {
		base := filepath.Base(match)
		// 从文件名中提取前缀：去掉 "_数字.zip" 部分
		parts := strings.Split(base, "_")
		if len(parts) >= 2 {
			prefix := strings.Join(parts[:len(parts)-1], "_")
			prefixes[prefix] = true
		}
	}

	var result []string
	for p := range prefixes {
		result = append(result, p)
	}
	sort.Strings(result)

	return result, nil
}

// NewExtractorApp 创建新的应用程序
func NewExtractorApp() *ExtractorApp {
	return &ExtractorApp{
		app:            app.New(),
		isExtracting:   false,
		extractorMutex: sync.Mutex{},
	}
}

// SetupUI 设置 UI 界面
func (ea *ExtractorApp) SetupUI() {
	ea.window = ea.app.NewWindow("数据包解压工具 v2.0")
	ea.window.Resize(fyne.NewSize(850, 680))

	// ===== 参数配置区域 =====
	ea.prefixSelect = widget.NewSelect([]string{}, func(s string) {})
	ea.prefixSelect.PlaceHolder = "自动检测或手动选择..."

	// 自动检测前缀
	prefixes, _ := AutoDetectPrefixes()
	ea.prefixSelect.Options = prefixes
	if len(prefixes) > 0 {
		ea.prefixSelect.SetSelected(prefixes[0])
	}

	// 输出目录选择
	currentDir, _ := os.Getwd()
	ea.outputEntry = widget.NewEntry()
	ea.outputEntry.SetText(currentDir)
	ea.outputEntry.PlaceHolder = "选择解压目录..."

	// MD5 验证选项
	ea.verifyCheck = widget.NewCheck("验证 MD5 完整性", func(b bool) {})
	ea.verifyCheck.Checked = false

	// ===== 日志显示区域（默认隐藏）=====
	ea.logText = widget.NewRichTextFromMarkdown("")
	ea.logText.Wrapping = fyne.TextWrapBreak

	logScroll := container.NewScroll(ea.logText)
	logScroll.SetMinSize(fyne.NewSize(800, 250))

	ea.logContainer = container.NewVBox(logScroll)
	ea.logCard = widget.NewCard("", "", ea.logContainer)

	// ===== 开始解压按钮（更明显）=====
	ea.startButton = widget.NewButton("▶ 开始解压", func() {
		ea.startExtraction()
	})

	// ===== 进度条区域 =====
	ea.progressBar = widget.NewProgressBar()
	ea.progressBar.Min = 0
	ea.progressBar.Max = 1
	ea.progressLabel = widget.NewLabel("就绪")

	// ===== 其他操作按钮 =====
	ea.toggleLogButton = widget.NewButton("显示日志", func() {
		ea.toggleLog()
	})

	// ===== 构建主界面 =====
	ea.updateMainContent()
}

// updateMainContent 更新主界面内容
func (ea *ExtractorApp) updateMainContent() {
	titleLabel := widget.NewRichTextFromMarkdown("## 📦 数据包解压工具")
	subtitleLabel := widget.NewLabel("Benagen 一键解压专用工具")
	titleContainer := container.NewCenter(
		container.NewVBox(
			titleLabel,
			subtitleLabel,
		),
	)

	prefixLabel := widget.NewLabel("数据包前缀")
	outputLabel := widget.NewLabel("输出目录")
	browseButton := widget.NewButton("浏览", func() {
		ea.browseOutputDir()
	})
	outputContainer := container.NewBorder(nil, nil, nil, browseButton, ea.outputEntry)

	configBox := container.NewVBox(
		container.NewPadded(
			container.NewVBox(
				prefixLabel,
				ea.prefixSelect,
			),
		),
		container.NewPadded(
			container.NewVBox(
				outputLabel,
				outputContainer,
			),
		),
		container.NewPadded(ea.verifyCheck),
	)

	startButtonBox := container.NewPadded(
		container.NewCenter(ea.startButton),
	)

	progressBox := container.NewPadded(
		container.NewVBox(
			ea.progressLabel,
			ea.progressBar,
		),
	)

	otherButtonBox := container.NewPadded(
		container.NewHBox(
			ea.toggleLogButton,
			widget.NewButton("清空日志", func() { ea.clearLog() }),
		),
	)

	var mainContent *fyne.Container
	if ea.logVisible {
		mainContent = container.NewVBox(
			container.NewPadded(titleContainer),
			widget.NewSeparator(),
			configBox,
			widget.NewSeparator(),
			startButtonBox,
			progressBox,
			otherButtonBox,
			widget.NewSeparator(),
			ea.logCard,
		)
		ea.window.SetContent(container.NewScroll(mainContent))
		ea.window.Resize(fyne.NewSize(850, 900))
	} else {
		mainContent = container.NewVBox(
			container.NewPadded(titleContainer),
			widget.NewSeparator(),
			configBox,
			widget.NewSeparator(),
			startButtonBox,
			progressBox,
			otherButtonBox,
		)
		ea.window.SetContent(mainContent)
		ea.window.Resize(fyne.NewSize(850, 680))
	}
}

// toggleLog 切换日志显示
func (ea *ExtractorApp) toggleLog() {
	ea.logVisible = !ea.logVisible
	if ea.logVisible {
		ea.toggleLogButton.SetText("隐藏日志")
	} else {
		ea.toggleLogButton.SetText("显示日志")
	}
	ea.updateMainContent()
}

// addLog 添加日志消息
func (ea *ExtractorApp) addLog(message string) {
	timestamp := time.Now().Format("15:04:05")
	logEntry := fmt.Sprintf("`[%s]` %s  \n", timestamp, message)
	currentText := ea.logText.String()
	ea.logText.ParseMarkdown(currentText + logEntry)
}

// clearLog 清空日志
func (ea *ExtractorApp) clearLog() {
	ea.logText.ParseMarkdown("")
}

// updateProgress 更新进度条
func (ea *ExtractorApp) updateProgress(current, total int, stage string) {
	if total > 0 {
		percentage := float64(current) / float64(total)
		ea.progressBar.SetValue(percentage)
		ea.progressLabel.SetText(fmt.Sprintf("%s: %d/%d", stage, current, total))
	}
}

// browseOutputDir 浏览输出目录
func (ea *ExtractorApp) browseOutputDir() {
	dialog.ShowFolderOpen(func(list fyne.ListableURI, err error) {
		if err == nil && list != nil {
			ea.outputEntry.SetText(list.Path())
		}
	}, ea.window)
}

// startExtraction 开始解压
func (ea *ExtractorApp) startExtraction() {
	ea.extractorMutex.Lock()
	if ea.isExtracting {
		ea.extractorMutex.Unlock()
		dialog.ShowError(fmt.Errorf("解压正在进行中，请稍候"), ea.window)
		return
	}
	ea.isExtracting = true
	ea.extractorMutex.Unlock()

	prefix := ea.prefixSelect.Selected
	outputDir := ea.outputEntry.Text
	verifyMD5 := ea.verifyCheck.Checked

	if prefix == "" {
		dialog.ShowError(fmt.Errorf("请选择数据包前缀"), ea.window)
		ea.extractorMutex.Lock()
		ea.isExtracting = false
		ea.extractorMutex.Unlock()
		return
	}

	if outputDir == "" {
		dialog.ShowError(fmt.Errorf("请选择输出目录"), ea.window)
		ea.extractorMutex.Lock()
		ea.isExtracting = false
		ea.extractorMutex.Unlock()
		return
	}

	ea.startButton.Disable()

	// 在后台线程执行解压
	go ea.extractionWorker(prefix, outputDir, verifyMD5)
}

// extractionWorker 解压工作线程
func (ea *ExtractorApp) extractionWorker(prefix, outputDir string, verifyMD5 bool) {
	defer func() {
		ea.extractorMutex.Lock()
		ea.isExtracting = false
		ea.extractorMutex.Unlock()
		ea.startButton.Enable()
	}()

	// 创建输出目录
	if err := os.MkdirAll(outputDir, 0755); err != nil {
		ea.addLog(fmt.Sprintf("[✗] 创建输出目录失败: %v", err))
		return
	}

	ea.addLog("[INFO] 开始解压...")
	ea.addLog(fmt.Sprintf("[INFO] 数据包前缀: %s", prefix))
	ea.addLog(fmt.Sprintf("[INFO] 输出目录: %s", outputDir))
	ea.addLog("")

	// 创建解压器
	extractor := &DataExtractor{
		prefix:    prefix,
		outputDir: outputDir,
	}

	// 查找文件
	chunks, err := extractor.FindChunks()
	if err != nil {
		ea.addLog(fmt.Sprintf("[✗] 查找文件失败: %v", err))
		return
	}

	if len(chunks) == 0 {
		ea.addLog("[✗] 没有找到 ZIP 文件")
		dialog.ShowError(fmt.Errorf("没有找到数据包文件"), ea.window)
		return
	}

	ea.addLog(fmt.Sprintf("[INFO] 找到 %d 个分块文件:", len(chunks)))
	for _, chunk := range chunks {
		ea.addLog(fmt.Sprintf("  - %s", filepath.Base(chunk)))
	}
	ea.addLog("")

	// 解压所有文件
	success := true
	for idx, chunk := range chunks {
		ea.addLog(fmt.Sprintf("[INFO] 正在解压第 %d/%d 个文件...", idx+1, len(chunks)))

		if err := extractor.ExtractChunk(chunk, func(current, total int) {
			ea.updateProgress(current, total, fmt.Sprintf("解压文件 (%d/%d)", idx+1, len(chunks)))
		}); err != nil {
			ea.addLog(fmt.Sprintf("[✗] 解压失败 %s: %v", filepath.Base(chunk), err))
			success = false
		} else {
			ea.addLog(fmt.Sprintf("[✓] 成功解压 %s", filepath.Base(chunk)))
		}

		ea.addLog("")
	}

	// 验证 MD5
	if verifyMD5 {
		ea.addLog("[INFO] 开始验证 MD5...")
		ea.addLog("")

		verifySuccess, verified, failed := extractor.VerifyMD5(prefix)
		ea.addLog("")
		ea.addLog(fmt.Sprintf("[INFO] MD5 验证完成: %d 个通过, %d 个失败", verified, failed))

		if !verifySuccess && failed > 0 {
			success = false
		}
	}

	// 显示结果
	ea.addLog("============================================================")
	if success {
		ea.addLog("[✓] 解压完成！")
		ea.addLog(fmt.Sprintf("[✓] 所有文件已解压到: %s", outputDir))
		
		// 在主线程中更新UI
		ea.progressBar.SetValue(1.0)
		ea.progressLabel.SetText("✅ 完成")
		
		// 简洁的成功对话框
		successLabel := widget.NewLabel("解压完成！")
		successDialog := dialog.NewCustom("", "确定", 
			container.NewPadded(
				container.NewCenter(successLabel),
			), ea.window)
		successDialog.Resize(fyne.NewSize(250, 100))
		successDialog.Show()
	} else {
		ea.addLog("[✗] 解压过程中出现错误")
		
		// 在主线程中更新UI
		ea.progressLabel.SetText("出现错误")
		errorLabel := widget.NewLabel("解压失败，请查看日志")
		errorDialog := dialog.NewCustom("", "确定",
			container.NewPadded(
				container.NewCenter(errorLabel),
			), ea.window)
		errorDialog.Resize(fyne.NewSize(250, 100))
		errorDialog.Show()
	}
}

// Run 运行应用程序
func (ea *ExtractorApp) Run() {
	ea.SetupUI()
	prefixes, _ := AutoDetectPrefixes()
	if len(prefixes) > 0 {
		ea.addLog(fmt.Sprintf("[✓] 自动检测到前缀: %s", strings.Join(prefixes, ", ")))
	} else {
		ea.addLog("[!] 未检测到数据包文件（*_*.zip）")
	}
	ea.window.ShowAndRun()
}

func main() {
	app := NewExtractorApp()
	app.Run()
}
