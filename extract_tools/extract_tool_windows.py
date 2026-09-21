#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Windows数据包解压工具 v1.0
功能：
  - GUI界面，双击即可运行
  - 自动检测数据包前缀（查找*_001.zip）
  - 解压所有ZIP文件
  - 验证MD5完整性
  - Windows原生GUI支持

使用方法：
  1. 将本脚本放在与ZIP文件相同的目录
  2. 直接双击运行
  3. 选择输出目录和MD5验证选项
  4. 点击"开始解压"即可

编译为EXE：
  pyinstaller --onefile --windowed --icon=icon.ico extract_tool_windows.py
"""

import os
import sys
import json
import hashlib
import zipfile
from pathlib import Path
from typing import List, Tuple, Optional
import tkinter as tk
from tkinter import ttk, filedialog, messagebox, scrolledtext
import threading
import time


class DataExtractor:
    """数据解压器"""
    
    def __init__(self, prefix: str, output_dir: str):
        self.prefix = prefix
        self.output_dir = Path(output_dir)
        self.chunks = []
        self.log_messages = []
    
    def add_log(self, message: str):
        """记录日志"""
        self.log_messages.append(message)
    
    def find_chunks(self) -> List[str]:
        """查找所有ZIP分块文件"""
        current_dir = Path.cwd()
        chunks = []
        
        # 查找所有matching的zip文件
        for file in sorted(current_dir.glob(f"{self.prefix}_*.zip")):
            chunks.append(str(file))
        
        self.chunks = chunks
        return chunks
    
    @staticmethod
    def calculate_md5(file_path: Path) -> str:
        """计算文件MD5"""
        hash_md5 = hashlib.md5()
        with open(file_path, "rb") as f:
            for chunk in iter(lambda: f.read(4096), b""):
                hash_md5.update(chunk)
        return hash_md5.hexdigest()
    
    def extract_chunk(self, chunk_file: str, progress_callback=None) -> bool:
        """解压单个chunk文件"""
        try:
            chunk_path = Path(chunk_file)
            self.add_log(f"[INFO] 解压 {chunk_path.name}...")
            
            with zipfile.ZipFile(chunk_path, 'r') as zf:
                # 获取总文件数用于进度条
                file_list = zf.namelist()
                total_files = len(file_list)
                
                for idx, file_info in enumerate(file_list):
                    try:
                        zf.extract(file_info, self.output_dir)
                        if progress_callback:
                            progress_callback(idx + 1, total_files)
                    except Exception as e:
                        self.add_log(f"[WARN] 解压文件 {file_info} 时出错: {e}")
            
            self.add_log(f"[✓] 成功解压 {chunk_path.name}")
            return True
        except Exception as e:
            self.add_log(f"[✗] 解压失败 {chunk_file}: {e}")
            return False
    
    def extract_all(self, progress_callback=None) -> bool:
        """解压所有文件"""
        if not self.chunks:
            self.add_log("[✗] 未找到ZIP文件")
            return False
        
        failed = 0
        for idx, chunk in enumerate(self.chunks):
            if not self.extract_chunk(chunk, progress_callback):
                failed += 1
        
        return failed == 0
    
    def verify_md5(self, prefix: str, progress_callback=None) -> Tuple[bool, int, int]:
        """验证MD5完整性"""
        md5_file = Path(prefix + ".md5")
        
        if not md5_file.exists():
            self.add_log(f"[!] MD5文件不存在: {md5_file}")
            return True, 0, 0  # 不认为是错误
        
        verified_count = 0
        failed_count = 0
        
        self.add_log(f"[INFO] 开始MD5验证...")
        
        try:
            with open(md5_file, 'r', encoding='utf-8') as f:
                lines = [line.strip() for line in f if line.strip() and not line.startswith('#')]
            
            total_lines = len(lines)
            
            for idx, line in enumerate(lines):
                if progress_callback:
                    progress_callback(idx + 1, total_lines, "MD5验证")
                
                parts = line.split(None, 1)
                if len(parts) != 2:
                    continue
                
                expected_hash = parts[0]
                filename = parts[1]
                
                file_path = Path(filename)
                
                # 如果是相对路径，相对于输出目录
                if not file_path.is_absolute():
                    file_path = self.output_dir / filename
                
                if not file_path.exists():
                    self.add_log(f"[!] 文件未找到: {filename}")
                    failed_count += 1
                    continue
                
                # 计算实际MD5
                actual_hash = self.calculate_md5(file_path)
                
                if expected_hash.lower() == actual_hash.lower():
                    self.add_log(f"[✓] {filename}")
                    verified_count += 1
                else:
                    self.add_log(f"[✗] {filename} (MD5不匹配!)")
                    self.add_log(f"    期望: {expected_hash}")
                    self.add_log(f"    实际: {actual_hash}")
                    failed_count += 1
        
        except Exception as e:
            self.add_log(f"[✗] MD5验证失败: {e}")
            return False, verified_count, failed_count
        
        self.add_log(f"[INFO] MD5验证完成: {verified_count}个通过, {failed_count}个失败")
        
        return failed_count == 0, verified_count, failed_count


class ExtractorGUI:
    """解压工具GUI界面"""
    
    def __init__(self, root):
        self.root = root
        self.root.title("数据包解压工具 v1.0")
        self.root.geometry("600x700")
        self.root.resizable(False, False)
        
        # 设置中文字体
        try:
            self.font_normal = ("Microsoft YaHei", 10)
            self.font_title = ("Microsoft YaHei", 12, "bold")
            self.font_mono = ("Courier New", 9)
        except:
            self.font_normal = ("Arial", 10)
            self.font_title = ("Arial", 12, "bold")
            self.font_mono = ("Courier New", 9)
        
        self.extractor = None
        self.extraction_thread = None
        self.is_extracting = False
        
        self.setup_ui()
        self.auto_detect_prefix()
    
    def setup_ui(self):
        """设置UI界面"""
        # 标题
        title_frame = ttk.Frame(self.root)
        title_frame.pack(pady=10, padx=10, fill='x')
        
        title_label = tk.Label(
            title_frame,
            text="数据包解压工具",
            font=self.font_title,
            fg="#0066cc"
        )
        title_label.pack()
        
        # 参数设置框架
        param_frame = ttk.LabelFrame(self.root, text="解压参数", padding=10)
        param_frame.pack(pady=10, padx=10, fill='x')
        
        # 前缀选择
        ttk.Label(param_frame, text="数据包前缀:", font=self.font_normal).grid(row=0, column=0, sticky='w', pady=5)
        self.prefix_var = tk.StringVar()
        self.prefix_combo = ttk.Combobox(
            param_frame,
            textvariable=self.prefix_var,
            font=self.font_normal,
            width=40,
            state='readonly'
        )
        self.prefix_combo.grid(row=0, column=1, sticky='w', padx=5, pady=5)
        
        # 输出目录选择
        ttk.Label(param_frame, text="输出目录:", font=self.font_normal).grid(row=1, column=0, sticky='w', pady=5)
        output_frame = ttk.Frame(param_frame)
        output_frame.grid(row=1, column=1, sticky='ew', padx=5, pady=5)
        
        self.output_var = tk.StringVar(value=str(Path.cwd()))
        output_entry = ttk.Entry(
            output_frame,
            textvariable=self.output_var,
            font=self.font_normal,
            width=35
        )
        output_entry.pack(side='left', fill='x', expand=True)
        
        browse_btn = ttk.Button(
            output_frame,
            text="浏览",
            command=self.browse_output_dir,
            width=10
        )
        browse_btn.pack(side='left', padx=5)
        
        # 验证选项
        self.verify_var = tk.BooleanVar(value=False)
        verify_check = ttk.Checkbutton(
            param_frame,
            text="验证MD5完整性（可选）",
            variable=self.verify_var,
            font=self.font_normal
        )
        verify_check.grid(row=2, column=0, columnspan=2, sticky='w', pady=5)
        
        # 分割线
        ttk.Separator(self.root, orient='horizontal').pack(fill='x', pady=10)
        
        # 日志框架
        log_frame = ttk.LabelFrame(self.root, text="执行日志", padding=5)
        log_frame.pack(pady=10, padx=10, fill='both', expand=True)
        
        self.log_text = scrolledtext.ScrolledText(
            log_frame,
            font=self.font_mono,
            height=15,
            width=70,
            bg="#f5f5f5",
            fg="#333333"
        )
        self.log_text.pack(fill='both', expand=True)
        
        # 进度条
        self.progress_var = tk.DoubleVar()
        self.progress_bar = ttk.Progressbar(
            self.root,
            variable=self.progress_var,
            maximum=100,
            mode='determinate'
        )
        self.progress_bar.pack(pady=5, padx=10, fill='x')
        
        self.progress_label = tk.Label(
            self.root,
            text="就绪",
            font=self.font_normal,
            fg="#666666"
        )
        self.progress_label.pack()
        
        # 按钮框架
        btn_frame = ttk.Frame(self.root)
        btn_frame.pack(pady=10, padx=10, fill='x')
        
        self.extract_btn = ttk.Button(
            btn_frame,
            text="开始解压",
            command=self.start_extraction,
            width=15
        )
        self.extract_btn.pack(side='left', padx=5)
        
        clear_btn = ttk.Button(
            btn_frame,
            text="清空日志",
            command=self.clear_log,
            width=15
        )
        clear_btn.pack(side='left', padx=5)
        
        exit_btn = ttk.Button(
            btn_frame,
            text="退出",
            command=self.root.quit,
            width=15
        )
        exit_btn.pack(side='left', padx=5)
    
    def auto_detect_prefix(self):
        """自动检测数据包前缀"""
        current_dir = Path.cwd()
        prefixes = set()
        
        # 查找所有_001.zip文件
        for file in current_dir.glob("*_001.zip"):
            prefix = file.name[:-8]  # 去掉_001.zip
            prefixes.add(prefix)
        
        if prefixes:
            prefixes_list = sorted(list(prefixes))
            self.prefix_combo['values'] = prefixes_list
            self.prefix_combo.set(prefixes_list[0])
            self.add_log(f"[✓] 自动检测到前缀: {', '.join(prefixes_list)}")
        else:
            self.prefix_combo['values'] = []
            self.add_log("[!] 未检测到数据包文件（*_001.zip）")
    
    def browse_output_dir(self):
        """浏览输出目录"""
        dir_path = filedialog.askdirectory(
            title="选择输出目录",
            initialdir=self.output_var.get()
        )
        if dir_path:
            self.output_var.set(dir_path)
    
    def add_log(self, message: str):
        """添加日志消息"""
        self.log_text.insert('end', message + '\n')
        self.log_text.see('end')
        self.root.update_idletasks()
    
    def clear_log(self):
        """清空日志"""
        self.log_text.delete('1.0', 'end')
    
    def update_progress(self, current: int, total: int, stage: str = "解压"):
        """更新进度条"""
        if total > 0:
            percentage = (current / total) * 100
            self.progress_var.set(percentage)
            self.progress_label.config(text=f"{stage}: {current}/{total}")
        self.root.update_idletasks()
    
    def start_extraction(self):
        """开始解压"""
        prefix = self.prefix_var.get()
        output_dir = self.output_var.get()
        verify_md5 = self.verify_var.get()
        
        if not prefix:
            messagebox.showerror("错误", "请选择数据包前缀")
            return
        
        if not output_dir:
            messagebox.showerror("错误", "请选择输出目录")
            return
        
        # 禁用按钮
        self.is_extracting = True
        self.extract_btn.config(state='disabled')
        self.prefix_combo.config(state='disabled')
        
        # 在线程中执行解压
        self.extraction_thread = threading.Thread(
            target=self._extraction_worker,
            args=(prefix, output_dir, verify_md5),
            daemon=True
        )
        self.extraction_thread.start()
    
    def _extraction_worker(self, prefix: str, output_dir: str, verify_md5: bool):
        """解压工作线程"""
        try:
            # 创建输出目录
            output_path = Path(output_dir)
            output_path.mkdir(parents=True, exist_ok=True)
            
            self.add_log(f"[INFO] 开始解压...")
            self.add_log(f"[INFO] 数据包前缀: {prefix}")
            self.add_log(f"[INFO] 输出目录: {output_dir}")
            self.add_log("")
            
            # 创建解压器
            self.extractor = DataExtractor(prefix, output_dir)
            
            # 查找文件
            chunks = self.extractor.find_chunks()
            self.add_log(f"[INFO] 找到 {len(chunks)} 个分块文件:")
            for chunk in chunks:
                self.add_log(f"  - {Path(chunk).name}")
            self.add_log("")
            
            if not chunks:
                self.add_log("[✗] 没有找到ZIP文件")
                messagebox.showerror("错误", "没有找到数据包文件")
                return
            
            # 解压所有文件
            success = True
            for idx, chunk in enumerate(chunks):
                self.add_log(f"[INFO] 正在解压第 {idx + 1}/{len(chunks)} 个文件...")
                
                def progress_callback(current, total):
                    self.update_progress(current, total, f"解压文件 ({idx + 1}/{len(chunks)})")
                
                if not self.extractor.extract_chunk(chunk, progress_callback):
                    success = False
                
                self.add_log("")
            
            # 验证MD5
            if verify_md5:
                self.add_log("[INFO] 开始验证MD5...")
                self.add_log("")
                
                def md5_progress(current, total, stage=""):
                    self.update_progress(current, total, stage)
                
                verify_success, verified, failed = self.extractor.verify_md5(prefix, md5_progress)
                self.add_log("")
                
                if not verify_success and failed > 0:
                    success = False
            
            # 显示结果
            self.add_log("=" * 60)
            if success:
                self.add_log("[✓] 解压完成！")
                self.add_log(f"[✓] 所有文件已解压到: {output_dir}")
                self.progress_var.set(100)
                self.progress_label.config(text="完成")
                messagebox.showinfo("成功", "解压完成！")
            else:
                self.add_log("[✗] 解压过程中出现错误")
                self.progress_label.config(text="出现错误")
                messagebox.showerror("失败", "解压过程中出现错误，请查看日志")
        
        except Exception as e:
            self.add_log(f"[✗] 异常错误: {e}")
            messagebox.showerror("异常", f"发生异常: {e}")
        
        finally:
            # 恢复按钮
            self.is_extracting = False
            self.extract_btn.config(state='normal')
            self.prefix_combo.config(state='readonly')


def main():
    """主函数"""
    root = tk.Tk()
    
    # 设置图标（如果存在）
    try:
        # 尝试加载图标
        icon_path = Path(__file__).parent / "icon.ico"
        if icon_path.exists():
            root.iconbitmap(str(icon_path))
    except:
        pass
    
    gui = ExtractorGUI(root)
    root.mainloop()


if __name__ == "__main__":
    main()
