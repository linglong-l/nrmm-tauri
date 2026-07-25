#!/usr/bin/env bash
#
# fix-appimage-env.sh
# AppImage 构建环境诊断与修复脚本（适用于 Linux / WSL2 / 原生发行版）
#
# 功能：
#   1. 检测 FUSE2/FUSE3 内核支持与用户态工具
#   2. 检测并下载 linuxdeploy + linuxdeploy-plugin-appimage 到本地缓存
#      （支持 GitHub 直连 + 国内 ghproxy 镜像回退）
#   3. 输出 Tauri bundler 需要设置的环境变量
#   4. 检测 glibc / libfuse 版本兼容性
#
# 用法：
#   bash scripts/fix-appimage-env.sh            # 诊断 + 修复，输出最终需要执行的 export 命令
#   bash scripts/fix-appimage-env.sh --apply    # 除诊断外，直接写入 ~/.bashrc 的 APPIMAGE 相关环境变量
#   bash scripts/fix-appimage-env.sh --check    # 仅诊断不做下载/修复
#
# 参考：
#   https://learn.microsoft.com/zh-cn/windows/wsl/wsl-config
#   https://docs.appimage.org/user-guide/troubleshooting/fuse.html
#   https://tauri.app/v1/guides/building/linux

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/nrmm-tauri/appimage-tools"
LOG_PREFIX="[fix-appimage]"
APPLY=false
CHECK_ONLY=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --apply)    APPLY=true; shift ;;
        --check)    CHECK_ONLY=true; shift ;;
        -h|--help)
            sed -n '2,30p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

PASS=0
WARN=0
FAIL=0

pass() { echo "$LOG_PREFIX ✅  $1"; PASS=$((PASS+1)); }
warn() { echo "$LOG_PREFIX ⚠️  $1"; WARN=$((WARN+1)); }
fail() { echo "$LOG_PREFIX ❌  $1"; FAIL=$((FAIL+1)); }
info() { echo "$LOG_PREFIX ℹ️  $1"; }

echo "=============================================="
echo "  nrmm-rust AppImage 构建环境诊断 & 修复"
echo "  项目根目录: $PROJECT_ROOT"
echo "  缓存目录:   $CACHE_DIR"
echo "=============================================="

# ============================================================
# 1. 发行版 & WSL 检测
# ============================================================
echo ""
echo "--- [1/7] 发行版与运行环境检测 ---"

IS_WSL=false
KERNEL_RELEASE=$(uname -r)
if [[ "$KERNEL_RELEASE" == *"microsoft"* || "$KERNEL_RELEASE" == *"WSL"* ]]; then
    IS_WSL=true
    pass "检测到 WSL2 环境 (kernel: $KERNEL_RELEASE)"
    if [ -f /proc/version ]; then
        info "WSL 版本信息: $(head -c 120 /proc/version)..."
    fi
else
    pass "原生 Linux 环境 (kernel: $KERNEL_RELEASE)"
fi

if [ -f /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    info "发行版: $PRETTY_NAME"
else
    warn "无法读取 /etc/os-release"
fi

ARCH=$(uname -m)
if [ "$ARCH" = "x86_64" ]; then
    pass "架构: $ARCH (支持)"
else
    fail "架构: $ARCH (Tauri bundler AppImage 仅在 x86_64 验证过)"
fi

# ============================================================
# 2. FUSE 支持检测（AppImage 运行的核心）
# ============================================================
echo ""
echo "--- [2/7] FUSE 内核模块与用户态工具检测 ---"

FUSE_OK=false
FUSE_DEV="/dev/fuse"

if [ -e "$FUSE_DEV" ]; then
    FUSE_MODE=$(stat -c '%a' "$FUSE_DEV" 2>/dev/null || echo "")
    pass "内核 FUSE 设备存在: $FUSE_DEV (mode: $FUSE_MODE)"

    FUSE_USER_GROUP=$(id)
    if [ -r "$FUSE_DEV" ] && [ -w "$FUSE_DEV" ]; then
        pass "当前用户对 /dev/fuse 有读写权限"
    else
        warn "当前用户对 /dev/fuse 权限不足，建议执行: sudo usermod -a -G fuse $(whoami)  然后重新登录"
    fi
    FUSE_OK=true
else
    if [ "$IS_WSL" = true ]; then
        fail "WSL2 环境中未检测到 /dev/fuse，需在 Windows 侧配置 .wslconfig 启用 fuse"
        echo ""
        echo "   ===== Windows 侧修复步骤 ===== "
        echo "   1) 以管理员身份运行 PowerShell"
        echo "   2) 执行: & scripts/setup-wsl2.ps1"
        echo "      （脚本会自动生成 %USERPROFILE%\\.wslconfig）"
        echo "   3) 执行: wsl --shutdown"
        echo "   4) 重新进入 WSL2，再次运行本脚本"
        echo ""
    else
        fail "未检测到 /dev/fuse，请安装 fuse: sudo dnf install fuse / sudo apt install fuse"
    fi
fi

# 检测用户态 fusermount 工具
if command -v fusermount >/dev/null 2>&1; then
    pass "fusermount 工具存在: $(command -v fusermount)"
elif command -v fusermount3 >/dev/null 2>&1; then
    pass "fusermount3 工具存在: $(command -v fusermount3)"
else
    warn "未找到 fusermount / fusermount3，可能导致 AppImage 无法挂载"
fi

# ============================================================
# 3. APPIMAGE_EXTRACT_AND_RUN 兜底方案（即使无 FUSE 也能构建）
# ============================================================
echo ""
echo "--- [3/7] 环境变量兜底方案 ---"

info "APPIMAGE_EXTRACT_AND_RUN=1 可以让 AppImage 工具自动解压运行，无需 FUSE"
if [ "${APPIMAGE_EXTRACT_AND_RUN:-}" = "1" ]; then
    pass "APPIMAGE_EXTRACT_AND_RUN 已设置为 1"
else
    warn "APPIMAGE_EXTRACT_AND_RUN 未设置（构建前建议 export APPIMAGE_EXTRACT_AND_RUN=1）"
fi

# ============================================================
# 4. 系统依赖检测（Tauri Linux 构建前置）
# ============================================================
echo ""
echo "--- [4/7] 系统构建依赖检测 ---"

check_pkg() {
    local name="$1"
    local bin="${2:-}"
    if [ -n "$bin" ] && command -v "$bin" >/dev/null 2>&1; then
        pass "$name: $bin 存在"
        return 0
    fi
    # 尝试用 dpkg / rpm 检测
    if command -v dpkg-query >/dev/null 2>&1; then
        if dpkg-query -W -f='${Status}' "$name" 2>/dev/null | grep -q "install ok installed"; then
            pass "$name: (dpkg) 已安装"
            return 0
        fi
    fi
    if command -v rpm >/dev/null 2>&1; then
        if rpm -q "$name" >/dev/null 2>&1; then
            pass "$name: (rpm) 已安装"
            return 0
        fi
    fi
    warn "$name: 未检测到（可能影响 deb/rpm 构建）"
    return 1
}

check_pkg "libwebkit2gtk-4.1-dev"
check_pkg "libgtk-3-dev"
check_pkg "patchelf" "patchelf"
check_pkg "openssl" "openssl"
check_pkg "rpm" "rpm"
check_pkg "dpkg-dev" "dpkg-deb"
check_pkg "file" "file"
check_pkg "curl" "curl"
check_pkg "wget" "wget"

# ============================================================
# 5. 检测 & 下载 linuxdeploy 工具链
# ============================================================
echo ""
echo "--- [5/7] linuxdeploy 工具链缓存 ---"

mkdir -p "$CACHE_DIR"

DOWNLOAD_URL_LINUXDEPLOY="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
DOWNLOAD_URL_PLUGIN="https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-x86_64.AppImage"
MIRROR_PREFIX="https://ghproxy.net/"

download_with_mirror() {
    local url="$1"
    local dest="$2"
    local tmp="${dest}.tmp"

    for try_url in \
        "${MIRROR_PREFIX}${url}" \
        "$url"
    ; do
        info "尝试下载: $try_url"
        if curl -fsSL --max-time 180 --connect-timeout 15 -o "$tmp" "$try_url"; then
            mv -f "$tmp" "$dest"
            chmod +x "$dest"
            pass "下载成功: $(basename "$dest")"
            return 0
        fi
        warn "下载失败: $(basename "$dest")  ($try_url)"
    done
    return 1
}

LINUXDEPLOY_BIN="$CACHE_DIR/linuxdeploy-x86_64.AppImage"
LINUXDEPLOY_PLUGIN_BIN="$CACHE_DIR/linuxdeploy-plugin-appimage-x86_64.AppImage"

if [ -f "$LINUXDEPLOY_BIN" ] && [ -x "$LINUXDEPLOY_BIN" ]; then
    pass "linuxdeploy 已缓存: $LINUXDEPLOY_BIN ($(du -h "$LINUXDEPLOY_BIN" | cut -f1))"
else
    if [ "$CHECK_ONLY" = true ]; then
        warn "linuxdeploy 未缓存（运行无 --check 参数自动下载）"
    else
        info "下载 linuxdeploy..."
        if download_with_mirror "$DOWNLOAD_URL_LINUXDEPLOY" "$LINUXDEPLOY_BIN"; then
            :
        else
            fail "linuxdeploy 下载失败，将依赖 Tauri bundler 内置版本"
        fi
    fi
fi

if [ -f "$LINUXDEPLOY_PLUGIN_BIN" ] && [ -x "$LINUXDEPLOY_PLUGIN_BIN" ]; then
    pass "linuxdeploy-plugin-appimage 已缓存: $LINUXDEPLOY_PLUGIN_BIN ($(du -h "$LINUXDEPLOY_PLUGIN_BIN" | cut -f1))"
else
    if [ "$CHECK_ONLY" = true ]; then
        warn "linuxdeploy-plugin-appimage 未缓存（运行无 --check 参数自动下载）"
    else
        info "下载 linuxdeploy-plugin-appimage..."
        if download_with_mirror "$DOWNLOAD_URL_PLUGIN" "$LINUXDEPLOY_PLUGIN_BIN"; then
            :
        else
            fail "plugin 下载失败，将依赖 Tauri bundler 内置版本"
        fi
    fi
fi

# 测试本地 linuxdeploy 能否运行（使用 APPIMAGE_EXTRACT_AND_RUN=1 兜底）
if [ -x "$LINUXDEPLOY_BIN" ]; then
    info "测试本地 linuxdeploy 是否可运行..."
    if APPIMAGE_EXTRACT_AND_RUN=1 "$LINUXDEPLOY_BIN" --appdir /tmp/nrmm-test-appdir.$$ --output appimage --help >/dev/null 2>&1 \
       || APPIMAGE_EXTRACT_AND_RUN=1 "$LINUXDEPLOY_BIN" --help >/dev/null 2>&1; then
        pass "本地 linuxdeploy 可正常运行"
    else
        warn "本地 linuxdeploy 无法执行（可能缺少依赖或 glibc 版本不兼容）"
        info "  尝试提取版本信息失败，可尝试用 --appimage-extract 手动解压测试"
    fi
fi

# ============================================================
# 6. 检测 glibc 版本（AppImage 跨发行版兼容性）
# ============================================================
echo ""
echo "--- [6/7] glibc / libstdc++ 版本检测 ---"

if command -v ldd >/dev/null 2>&1; then
    GLIBC_VER=$(ldd --version 2>&1 | head -1 | grep -oP '\d+\.\d+' | head -1)
    info "glibc 版本: $GLIBC_VER"
    if [ -n "$GLIBC_VER" ]; then
        # Tauri AppImage 建议在 glibc <= 2.35 的系统上构建，兼容性最好（如 Ubuntu 22.04）
        MAJOR="${GLIBC_VER%%.*}"
        MINOR="${GLIBC_VER#*.}"
        if [ "$MAJOR" -ge 2 ] && [ "$MINOR" -le 35 ]; then
            pass "glibc 版本 $GLIBC_VER 适合构建高兼容性 AppImage"
        else
            warn "glibc 版本 $GLIBC_VER 较新，生成的 AppImage 可能无法在老系统（如 Ubuntu 20.04 / CentOS 7）上运行"
            info "  如追求最大兼容性，可在 Ubuntu 22.04 环境中构建（本项目 CI 已使用 ubuntu-latest）"
        fi
    fi
fi

# ============================================================
# 7. 总结 & 输出环境变量
# ============================================================
echo ""
echo "--- [7/7] 总结 & 执行建议 ---"

EXPORT_BLOCK="# ===== nrmm-rust AppImage 构建环境变量（fix-appimage-env.sh 自动生成） =====
export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH=x86_64
# ===== 关键：Fedora 39+ / Ubuntu 24.04+ glibc >=2.38 默认使用 DT_RELR (.relr.dyn 段 type=0x13) =====
# linuxdeploy 内置的 strip binutils 非常古老（~2020 年），根本不认识 .relr.dyn，导致所有库 strip 失败 -> 最终 failed to run linuxdeploy
# 方案A（推荐，零副作用）：直接跳过 linuxdeploy 的 strip 调用。系统库和 Rust 可执行文件本项目已在 Cargo.toml profile.release.strip=true 阶段剥离过，再 strip 一次是多余操作。
export NO_STRIP=1
# 方案B（如果不希望放弃 strip）：强制使用系统本地的 binutils strip，它一定认识当前系统库生成的 ELF 段格式。
#   如果 NO_STRIP=1 生效则这行会被忽略，两者共存无害。
export STRIP=$(command -v strip || echo /usr/bin/strip)
"
if [ -f "$LINUXDEPLOY_BIN" ] && [ -x "$LINUXDEPLOY_BIN" ]; then
    EXPORT_BLOCK+="export TAURI_BUNDLER_LINUXDEPLOY_BINARY=\"$LINUXDEPLOY_BIN\"
"
fi
if [ -f "$LINUXDEPLOY_PLUGIN_BIN" ] && [ -x "$LINUXDEPLOY_PLUGIN_BIN" ]; then
    EXPORT_BLOCK+="export TAURI_BUNDLER_APPDIR_PLUGIN_BINARY=\"$LINUXDEPLOY_PLUGIN_BIN\"
"
fi
EXPORT_BLOCK+="# ======================================================================="

echo ""
echo "================== 构建前需要执行的环境变量（复制粘贴） ==================="
echo "$EXPORT_BLOCK"
echo "=========================================================================="

if [ "$APPLY" = true ]; then
    MARKER="# nrmm-tauri-appimage-env"
    TARGET_RC=""
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
        if [ -f "$rc" ]; then TARGET_RC="$rc"; break; fi
    done
    if [ -n "$TARGET_RC" ]; then
        info "写入环境变量到 $TARGET_RC (marker: $MARKER)"
        # 去除旧的
        if grep -q "$MARKER" "$TARGET_RC" 2>/dev/null; then
            sed -i "/${MARKER}-BEGIN/,/${MARKER}-END/d" "$TARGET_RC"
        fi
        {
            echo ""
            echo "$MARKER-BEGIN"
            echo "$EXPORT_BLOCK"
            echo "$MARKER-END"
        } >> "$TARGET_RC"
        pass "已写入 $TARGET_RC，下次登录自动生效（或 source $TARGET_RC 立即生效）"
    else
        warn "未找到 .bashrc/.zshrc，未写入持久化配置（可手动把上面 export 命令加入 shell 配置）"
    fi
fi

echo ""
echo "=============================================="
echo "  通过: $PASS  警告: $WARN  失败: $FAIL"
echo "=============================================="

if [ "$FAIL" -gt 0 ]; then
    echo ""
    fail "存在 $FAIL 个失败项，请先修复后再执行 Tauri AppImage 构建"
    echo "  快速构建命令（修复后执行）:"
    echo "    cd $PROJECT_ROOT"
    echo "    $EXPORT_BLOCK"
    echo "    npm run tauri build -- --target x86_64-unknown-linux-gnu --bundles appimage,deb,rpm"
    exit 1
fi

echo ""
info "本地环境就绪。如果仍遇到 AppImage 构建问题，可使用便携版 tar.gz 作为兜底方案:"
info "  node scripts/build-portable.mjs --target x86_64-unknown-linux-gnu -o dist-linux"
exit 0
