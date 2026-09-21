#!/bin/bash
##############################################################################
# Windows EXE 编译脚本（Linux/Mac）
# 功能：在 Linux/Mac 环境下交叉编译 Go 代码为 Windows EXE
# 
# 支持两种编译方式：
#   1. Go 版本编译（推荐）- extract_tool_windows.go
#   2. Python 版本编译 - extract_tool_windows.py + PyInstaller
#
# 使用方法：
#   bash build_windows_exe_linux.sh go      # 编译 Go 版本
#   bash build_windows_exe_linux.sh python  # 编译 Python 版本
#   bash build_windows_exe_linux.sh         # 默认编译 Go 版本
#
##############################################################################

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# 打印函数
print_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

print_error() {
    echo -e "${RED}[✗]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

# 获取编译方式
BUILD_TYPE="${1:-go}"

# 验证编译方式
if [ "$BUILD_TYPE" != "go" ] && [ "$BUILD_TYPE" != "python" ]; then
    print_error "无效的编译方式: $BUILD_TYPE"
    echo "使用方法: bash build_windows_exe_linux.sh [go|python]"
    exit 1
fi

echo ""
echo "=========================================="
echo "Windows EXE 编译脚本（$BUILD_TYPE 版本）"
echo "=========================================="
echo ""

# ========== Go 版本编译 ==========
if [ "$BUILD_TYPE" = "go" ]; then
    print_info "检查 Go 环境..."
    
    if ! command -v go &> /dev/null; then
        print_error "Go 未安装"
        echo ""
        echo "请先安装 Go 1.16 或更新版本："
        echo "  Ubuntu/Debian: sudo apt-get install golang-go"
        echo "  CentOS/RHEL:   sudo yum install golang"
        echo "  macOS:         brew install go"
        echo ""
        echo "或访问: https://golang.org/dl"
        exit 1
    fi
    
    GO_VERSION=$(go version | awk '{print $3}')
    print_success "Go 版本: $GO_VERSION"
    echo ""
    
    # 检查源文件
    if [ ! -f "extract_tool_windows.go" ]; then
        print_error "找不到源文件: extract_tool_windows.go"
        exit 1
    fi
    
    print_info "检查依赖..."
    
    # 初始化 Go Module（如果不存在）
    if [ ! -f "go.mod" ]; then
        print_info "初始化 Go Module..."
        go mod init tosup 2>/dev/null || true
    fi
    
    # 下载 Fyne 依赖
    print_info "下载 Fyne 依赖..."
    go get fyne.io/fyne/v2@latest
    
    # 创建输出目录
    mkdir -p dist_linux
    
    print_info "开始编译..."
    echo ""
    
    # 编译参数说明
    # GOOS=windows    - 目标操作系统为 Windows
    # GOARCH=amd64    - 目标架构为 64 位
    # -o              - 输出文件
    # -ldflags        - 链接器标志
    # -s -w           - 移除调试符号，减少文件大小
    
    export GOOS=windows
    export GOARCH=amd64
    export CGO_ENABLED=0
    
    OUTPUT_FILE="dist_linux/数据包解压工具.exe"
    
    # 编译
    if go build -o "$OUTPUT_FILE" -ldflags="-s -w" extract_tool_windows.go; then
        print_success "编译完成"
    else
        print_error "编译失败"
        exit 1
    fi
    
    # 检查输出文件
    if [ -f "$OUTPUT_FILE" ]; then
        FILE_SIZE=$(du -h "$OUTPUT_FILE" | cut -f1)
        print_success "生成文件: $OUTPUT_FILE"
        print_success "文件大小: $FILE_SIZE"
    else
        print_error "输出文件不存在"
        exit 1
    fi
    
    echo ""
    print_success "编译完成！"
    echo ""
    echo "使用步骤："
    echo "  1. 复制 $OUTPUT_FILE 到 Windows 机器"
    echo "  2. 将 EXE 放在与 ZIP 文件相同目录"
    echo "  3. 双击运行"
    echo ""
    
# ========== Python 版本编译 ==========
elif [ "$BUILD_TYPE" = "python" ]; then
    print_info "检查 Python 环境..."
    
    if ! command -v python3 &> /dev/null; then
        print_error "Python3 未安装"
        echo ""
        echo "请先安装 Python 3.8 或更新版本：" 
        echo "  Ubuntu/Debian: sudo apt-get install python3"
        echo "  CentOS/RHEL:   sudo yum install python3"
        echo "  macOS:         brew install python3"
        exit 1
    fi
    
    PYTHON_VERSION=$(python3 --version)
    print_success "$PYTHON_VERSION"
    echo ""
    
    # 检查源文件
    if [ ! -f "extract_tool_windows.py" ]; then
        print_error "找不到源文件: extract_tool_windows.py"
        exit 1
    fi
    
    print_info "安装 PyInstaller..."
    pip3 install pyinstaller -q
    
    # 创建输出目录
    mkdir -p dist_linux
    rm -rf dist_linux/*
    
    print_info "开始编译..."
    echo ""
    
    # 编译参数说明
    # --onefile       - 打包为单个 EXE 文件
    # --windowed      - GUI 应用（无控制台）
    # --name          - 程序名称
    # --distpath      - 输出目录
    # --specpath      - 规范文件输出目录
    # -i              - 图标文件（可选）
    
    pyinstaller \
        --onefile \
        --windowed \
        --name "数据包解压工具" \
        --distpath "./dist_linux" \
        --specpath "./dist_linux" \
        --noconfirm \
        extract_tool_windows.py

    # 检查输出文件（PyInstaller 在 Linux 上生成 Linux 可执行文件，不带 .exe 扩展名）
    OUTPUT_FILE="dist_linux/数据包解压工具"
    if [ -f "$OUTPUT_FILE" ]; then
        FILE_SIZE=$(du -h "$OUTPUT_FILE" | cut -f1)
        print_success "生成文件: $OUTPUT_FILE"
        print_success "文件大小: $FILE_SIZE"
        # 验证文件类型
        print_info "验证文件类型..."
        if file "$OUTPUT_FILE" | grep -q "ELF"; then
            print_success "成功生成 Linux 可执行文件"
            print_warning "注意：在 Linux 上编译 Python 应用生成的是 Linux 可执行文件"
            print_warning "如需 Windows EXE，请使用 Go 版本编译或使用 Windows 系统"
        else
            print_warning "生成的文件类型未知"
        fi
    else
        print_error "编译失败 - 未生成可执行文件"
        exit 1
    fi
    
    echo ""
    print_success "编译完成！"
    echo ""
    echo "使用步骤："
    echo "  1. 复制 $OUTPUT_FILE 到 Windows 机器"
    echo "  2. 将可执行文件放在与 ZIP 文件相同目录"
    echo "  3. 在 Linux 系统中运行"
    echo ""
fi

echo "=========================================="
echo "编译完成"
echo "=========================================="
