#!/bin/bash
################################################################################
# 数据包解压工具 - 纯Bash脚本版本
# 功能：自动解压所有ZIP文件，默认不验证MD5，需要时使用--md5参数
# 
# 使用方法：
#   bash extract_tool.sh [数据包前缀] [输出目录] [--md5]
#
# 示例：
#   bash extract_tool.sh data                    # 解压data_*.zip到当前目录，不验证MD5
#   bash extract_tool.sh data /restore/path      # 解压到指定目录，不验证MD5
#   bash extract_tool.sh data /restore/path --md5 # 解压到指定目录并启用MD5验证
#
################################################################################

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 打印带颜色的消息
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

# 检查命令是否存在
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# 解析命令行参数
VERIFY_MD5="false"
SHOW_HELP=false
PREFIX=""
OUTPUT_DIR="."

# 解析可选参数
while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            SHOW_HELP=true
            shift
            ;;
        --md5)
            VERIFY_MD5="true"
            shift
            ;;
        -*)
            echo "Unknown option: $1"
            exit 1
            ;;
        *)
            # 第一个非选项参数是前缀
            if [ -z "$PREFIX" ]; then
                PREFIX="$1"
            elif [ -z "$OUTPUT_DIR" ] || [ "$OUTPUT_DIR" = "." ]; then
                OUTPUT_DIR="$1"
            else
                echo "Too many arguments"
                exit 1
            fi
            shift
            ;;
    esac
done

if [ "$SHOW_HELP" = "true" ]; then
    echo "Usage: $0 [prefix] [output_dir] [--md5]"
    echo ""
    echo "Auto-detection mode (default):"
    echo "  Just run: $0"
    echo "  Script will auto-detect data package prefix"
    echo ""
    echo "Manual mode:"
    echo "  $0 data                         # Use 'data' as prefix"
    echo "  $0 data /restore/path           # Specify output directory"
    echo "  $0 data /restore/path --md5     # Enable MD5 verification"
    exit 0
fi

# 自动检测前缀（查找*_001.zip文件）
if [ -z "$PREFIX" ]; then
    # 在当前目录查找ZIP文件
    for zip_file in *_001.zip; do
        if [ -f "$zip_file" ]; then
            # 提取前缀（去掉_001.zip）
            PREFIX="${zip_file%_001.zip}"
            break
        fi
    done
    
    if [ -z "$PREFIX" ]; then
        print_error "无法自动检测数据包前缀"
        echo "请在与ZIP文件相同的目录中运行此脚本，或手动指定前缀："
        echo "  $0 data"
        exit 1
    fi
fi

# 检查必要的工具
if ! command_exists unzip; then
    print_error "unzip command not found. Please install unzip first."
    echo "  Ubuntu/Debian: sudo apt-get install unzip"
    echo "  CentOS/RHEL:   sudo yum install unzip"
    echo "  macOS:         brew install unzip"
    exit 1
fi

print_info "Starting data extraction..."
print_info "Data prefix: $PREFIX"
print_info "Output directory: $OUTPUT_DIR"
print_info "MD5 verification: $VERIFY_MD5"
echo ""

# 创建输出目录
if [ ! -d "$OUTPUT_DIR" ]; then
    print_info "Creating output directory: $OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
fi

# 查找所有ZIP文件
CHUNKS=()
for chunk in ${PREFIX}_*.zip; do
    if [ -f "$chunk" ]; then
        CHUNKS+=("$chunk")
    fi
done

if [ ${#CHUNKS[@]} -eq 0 ]; then
    print_error "No chunk files found matching pattern: ${PREFIX}_*.zip"
    echo "Current directory: $(pwd)"
    echo "Available files:"
    ls -lh *.zip 2>/dev/null || echo "  (no ZIP files found)"
    exit 1
fi

print_info "Found ${#CHUNKS[@]} chunk file(s)"
echo ""

# 解压所有chunk
EXTRACTION_FAILED=0
for chunk in "${CHUNKS[@]}"; do
    print_info "Extracting $chunk..."
    
    if unzip -q "$chunk" -d "$OUTPUT_DIR"; then
        print_success "Extracted $chunk"
    else
        print_error "Failed to extract $chunk"
        EXTRACTION_FAILED=1
    fi
done

echo ""

# 验证MD5
if [ "$VERIFY_MD5" = "true" ]; then
    print_info "Verifying MD5 checksums..."
    echo ""
    
    MD5_FILE="${PREFIX}.md5"
    if [ ! -f "$MD5_FILE" ]; then
        print_warning "MD5 file not found: $MD5_FILE"
        print_warning "Skipping MD5 verification"
    else
        FAILED_MD5=0
        VERIFIED_COUNT=0
        
        while IFS= read -r line; do
            # 跳过空行和注释
            if [ -z "$line" ] || [[ "$line" =~ ^# ]]; then
                continue
            fi
            
            # 解析MD5行：hash  filename
            expected_hash=$(echo "$line" | awk '{print $1}')
            filename=$(echo "$line" | awk '{print $2}')
            
            if [ -z "$filename" ]; then
                continue
            fi
            
            # 检查文件是否存在
            if [ ! -f "$filename" ]; then
                print_warning "File not found for verification: $filename"
                continue
            fi
            
            # 计算实际MD5
            if command_exists md5sum; then
                actual_hash=$(md5sum "$filename" | awk '{print $1}')
            elif command_exists md5; then
                # macOS
                actual_hash=$(md5 -q "$filename")
            else
                print_warning "Neither md5sum nor md5 command found, skipping hash verification"
                continue
            fi
            
            # 比较MD5
            if [ "$expected_hash" = "$actual_hash" ]; then
                print_success "$filename"
                VERIFIED_COUNT=$((VERIFIED_COUNT + 1))
            else
                print_error "$filename (hash mismatch!)"
                echo "  Expected: $expected_hash"
                echo "  Actual:   $actual_hash"
                FAILED_MD5=$((FAILED_MD5 + 1))
            fi
        done < "$MD5_FILE"
        
        echo ""
        print_info "MD5 verification complete: $VERIFIED_COUNT verified, $FAILED_MD5 failed"
        
        if [ $FAILED_MD5 -gt 0 ]; then
            EXTRACTION_FAILED=1
        fi
    fi
fi

echo ""

# 最终结果
if [ $EXTRACTION_FAILED -eq 0 ]; then
    print_success "Extraction completed successfully!"
    if [ "$OUTPUT_DIR" = "." ]; then
        print_info "Extracted files are in: $(pwd)"
    else
        print_info "Extracted files are in: $(cd "$OUTPUT_DIR" && pwd)"
    fi
    exit 0
else
    print_error "Extraction completed with errors!"
    echo ""
    echo "Troubleshooting:"
    echo "  1. Check disk space: df -h"
    echo "  2. Check file permissions: chmod -R 755 $OUTPUT_DIR"
    echo "  3. Verify ZIP files are not corrupted: unzip -t ${CHUNKS[0]}"
    exit 1
fi
