#!/usr/bin/env bash
# ============================================================================
# build-linux-gnu.sh — NRMM Linux 手动构建脚本（gnu 原生工具链，Ubuntu 22.04）
#
# 用途：
#   在 Ubuntu 22.04（或任何 glibc >= 2.35 的宿主机）上用系统 gcc/clang 原生构建
#   Linux 发行包：deb / rpm / portable tar.gz（免安装包内置 7z CLI），
#   并审计产物 glibc 符号版本。
#
# 设计要点：
#   - 使用宿主机原生工具链（gcc/clang）。在 Ubuntu 22.04 上，宿主 gcc 默认产 glibc 2.35
#     符号，天然满足项目下限（这是唯一官方支持的 Linux 构建方式，不引入 zig/交叉编译）。
#   - 风险：若宿主机 glibc > 2.35（如 24.04→2.39），产物会击穿兼容下限；
#     故本脚本保留 glibc 审计步骤，构建失败会直接报错提示。CI 用 ubuntu:22.04 容器规避。
#   - 输出归入 dist-linux/；产物文件名改写 nrmm-tauri_<版本>_x64-Linux.*。
#
# 前置依赖（Ubuntu 22.04 侧）：
#   rustup + stable、node >= 22、objdump（binutils）、patchelf、rpm、fakeroot、
#   libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
#   libsoup-3.0-dev libssl-dev pkg-config
#
# 用法：
#   bash scripts/build-linux-gnu.sh                 # 默认 x86_64，输出 dist-linux/
# ============================================================================
set -euo pipefail

# ---------- 可配置项（环境变量可覆盖） ----------
TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}"
GLIBC_VERSION="${GLIBC_VERSION:-2.35}"   # 项目下限：Ubuntu 22.04 = glibc 2.35
DIST_DIR="${DIST_DIR:-dist-linux}"
ARCHIVE_MIRROR="${ARCHIVE_MIRROR:-https://www.7-zip.org/a}"

cd "$(dirname "$0")/.."

# ---------- 0. 版本号（命名规范根基，与 package.json 一致） ----------
VERSION="$(node -p "require('./package.json').version" 2>/dev/null || echo "0.0.0")"

# ---------- 1. 前置检查 ----------
fail() { echo "[build-linux-gnu] ERROR: $*" >&2; exit 1; }

command -v cargo >/dev/null 2>&1 || fail "cargo 不在 PATH（rustup 安装 stable）"
command -v node  >/dev/null 2>&1 || fail "node 不在 PATH（需 >= 22，建议 fnm 管理）"
command -v objdump >/dev/null 2>&1 || fail "objdump 不在 PATH（apt install binutils）"
command -v patchelf >/dev/null 2>&1 || fail "patchelf 不在 PATH（apt install patchelf）"
command -v rpm  >/dev/null 2>&1 || fail "rpm 不在 PATH（apt install rpm，用于 rpm 打包）"
command -v dpkg >/dev/null 2>&1 || fail "dpkg 不在 PATH（apt install dpkg，用于 deb 打包）"
command -v fakeroot >/dev/null 2>&1 || fail "fakeroot 不在 PATH（apt install fakeroot）"
command -v g++     >/dev/null 2>&1 || fail "g++ 不在 PATH（apt install build-essential；unrar 是 C++，需 g++ 与 libstdc++-dev）"

# 仅支持 x86_64
case "$TARGET_TRIPLE" in
  x86_64-*) ;;
  *) fail "不支持的架构: $TARGET_TRIPLE（项目约束：仅 x86_64，不做交叉编译）" ;;
esac

echo "[build-linux-gnu] 原生 gcc 轨 | target=$TARGET_TRIPLE | glibc<=$GLIBC_VERSION | 输出=$DIST_DIR | 版本=$VERSION"

rustup target list --installed | grep -q "^$TARGET_TRIPLE" || {
  echo "[build-linux-gnu] 安装 Rust target: $TARGET_TRIPLE"
  rustup target add "$TARGET_TRIPLE"
}

# ---------- 2. 7z Linux sidecar 检查/下载（目录被 gitignore，新检出环境需自补） ----------
SEVENZIP_TAR="7z2602-linux-x64.tar.xz"
SEVENZIP_SRC="src-tauri/binaries/7z/7z2602-linux-x64"
if [ ! -f "$SEVENZIP_SRC/7zz" ]; then
  echo "[build-linux-gnu] 下载 7z Linux sidecar ($SEVENZIP_TAR)..."
  mkdir -p "$SEVENZIP_SRC"
  curl -fSL --retry 3 -o "/tmp/$SEVENZIP_TAR" "$ARCHIVE_MIRROR/$SEVENZIP_TAR" \
    || fail "下载 $SEVENZIP_TAR 失败（可用 ARCHIVE_MIRROR 换镜像），或手动放置该包并解压后将其 7zz 放到 $SEVENZIP_SRC/ 重试"
  tar -xJf "/tmp/$SEVENZIP_TAR" -C "$SEVENZIP_SRC"
  rm -f "/tmp/$SEVENZIP_TAR"
  SRC_FILE="$(find "$SEVENZIP_SRC" -type f -name '7zz' | head -1 || true)"
  [ -n "$SRC_FILE" ] && cp "$SRC_FILE" "$SEVENZIP_SRC/7zz"
fi
[ -f "$SEVENZIP_SRC/7zz" ] || fail "7z sidecar 不完整：$SEVENZIP_SRC/7zz 缺失"
echo "[build-linux-gnu] 7z sidecar ready: $SEVENZIP_SRC/7zz (x86_64, 动态 7zz)"

# ---------- 3. 前端依赖 ----------
if [ ! -d node_modules ]; then
  echo "[build-linux-gnu] 安装 npm 依赖..."
  npm ci
fi

# ---------- 4. Tauri 构建（deb/rpm，原生 gcc 工具链） ----------
# 注意：不得用 `npm run tauri build -- --target X`——npm 会把 `--target` 当本身配置吞掉。
# 统一用 npx tauri 直传。
#
# 已取消 AppImage 支持：Tauri 强依赖 WebKit/glibc 等无法静态链接的庞大 C/C++ 库，
# AppImage 的「单文件便携」优势在此不成立，且打包器需从 GitHub 拉取 runtime（国内网络易失败）。
# 改用 deb/rpm + 自带 .7z 的 portable tar.gz（见步骤 5）覆盖安装与便携需求。
echo "[build-linux-gnu] 开始 tauri build（bundler: deb/rpm）..."
npx tauri build --target "$TARGET_TRIPLE" --bundles deb,rpm

# ---------- 5. portable tar.gz（免安装包一律内置 7z CLI，保证完整归档格式支持） ----------
# 注：不再产出 --no-7z 单程序版；该形态仅由使用者自行抉择时手动调用，异常自行承担。
node scripts/build-portable.mjs --target "$TARGET_TRIPLE" -o "$DIST_DIR"

# ---------- 6. glibc 符号版本审计（防宿主 glibc 过高击穿下限） ----------
detect_release_dir() {
  for d in "target/$TARGET_TRIPLE/release" "src-tauri/target/$TARGET_TRIPLE/release"; do
    [ -d "$d" ] && { echo "$d"; return; }
  done
  fail "找不到 release 目录（target/ 与 src-tauri/target/ 均无）"
}
RELEASE_DIR="$(detect_release_dir)"
BIN="$RELEASE_DIR/nrmm-tauri"
[ -f "$BIN" ] || fail "主二进制不存在: $BIN"

MAX_GLIBC="$(objdump -T "$BIN" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -uV | tail -1 || true)"
echo "[build-linux-gnu] 产物最大 glibc 符号版本: ${MAX_GLIBC:-无（静态/无符号依赖）}"
if [ -n "$MAX_GLIBC" ]; then
  NEWER="$(printf '%s\n%s\n' "$GLIBC_VERSION" "$MAX_GLIBC" | sort -V | tail -1)"
  if [ "$NEWER" != "$GLIBC_VERSION" ]; then
    fail "glibc 审计失败：产物依赖 $MAX_GLIBC > 目标 $GLIBC_VERSION（宿主 glibc 过高；请换 Ubuntu 22.04 构建或改用 ubuntu:22.04 容器）"
  fi
fi
echo "[build-linux-gnu] glibc 审计通过（<= $GLIBC_VERSION）"

# ---------- 7. 汇总产物（bundler 出厂名 -> 规范名） ----------
BUNDLE_DIR="$RELEASE_DIR/bundle"
mkdir -p "$DIST_DIR"
rename_to_spec() {  # rename_to_spec <src>
  local src="$1"; [ -f "$src" ] || { echo "  [WARN] 未找到: $src"; return; }
  local bn; bn="$(basename "$src")"
  local dest
  case "$bn" in
    *.deb)      dest="nrmm-tauri_${VERSION}_x64-Linux.deb" ;;
    *.rpm)      dest="nrmm-tauri_${VERSION}_x64-Linux.rpm" ;;
    *)          dest="$bn" ;;
  esac
  cp "$src" "$DIST_DIR/$dest"
  echo "  [OK] $dest"
}
echo "[build-linux-gnu] 收集并改名产物到 $DIST_DIR/ ..."
rename_to_spec "$(find "$BUNDLE_DIR/deb"      -maxdepth 1 -name '*.deb'      2>/dev/null |  head -1)"
rename_to_spec "$(find "$BUNDLE_DIR/rpm"      -maxdepth 1 -name '*.rpm'      2>/dev/null | head -1)"
[ -f "$DIST_DIR/latest-linux.json" ] || cp "$(find "$BUNDLE_DIR" -maxdepth 2 -name 'latest.json' 2>/dev/null | head -1)" "$DIST_DIR/latest-linux.json" 2>/dev/null || true

echo ""
echo "===== 构建完成（gnu 轨）====="
ls -la "$DIST_DIR/"
echo "请手动上传以上产物到 Gitee Release（tag: v$VERSION）。"
