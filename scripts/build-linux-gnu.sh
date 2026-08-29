#!/usr/bin/env bash
# ============================================================================
# build-linux-gnu.sh — NRMM Linux 手动构建脚本（gnu 原生工具链，Ubuntu 22.04）
#
# 用途：
#   在 Ubuntu 22.04（或任何 glibc >= 2.35 的宿主机）上用系统 gcc/clang 原生构建
#   Linux 发行包：AppImage / deb / rpm / portable tar.gz（含 7z 与单程序 no7z 两版），
#   并审计产物 glibc 符号版本。
#
# 与 build-linux-zig.sh 的区别：
#   - 不引入 zig，直接使用宿主机原生工具链。在 Ubuntu 22.04 上，
#     宿主 gcc 默认产 glibc 2.35 符号，天然满足项目下限（无需交叉/锁版本）。
#   - 风险：若宿主机 glibc > 2.35（如 24.04→2.39），产物会击穿兼容下限。
#     故本脚本保留 glibc 审计步骤，构建失败会直接报错提示。
#   - 输出归入 dist-linux/gnu/；产物文件名改写 nrmm-tauri_<版本>_x64-Linux.*（与 zig 轨同名，
#     发布时分别放入 Release 的 linux-gnu/ 与 linux-zig/ 子目录避免冲突）。
#
# 前置依赖（Ubuntu 22.04 侧）：
#   rustup + stable、node >= 22、objdump（binutils）、patchelf、rpm、fakeroot、
#   libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
#   libsoup-3.0-dev libssl-dev pkg-config
#
# 用法：
#   bash scripts/build-linux-gnu.sh                 # 默认 x86_64，输出 dist-linux/gnu/
# ============================================================================
set -euo pipefail

# ---------- 可配置项（环境变量可覆盖） ----------
TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}"
GLIBC_VERSION="${GLIBC_VERSION:-2.35}"   # 项目下限：Ubuntu 22.04 = glibc 2.35
DIST_DIR="${DIST_DIR:-dist-linux/gnu}"
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
command -v fakeroot >/dev/null 2>&1 || fail "fakeroot 不在 PATH（apt install fakeroot）"

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

# ---------- 4. Tauri 构建（AppImage/deb/rpm，原生 gcc 工具链） ----------
# 注意：不得用 `npm run tauri build -- --target X`——npm 会把 `--target` 当本身配置吞掉。
# 统一用 npx tauri 直传。
echo "[build-linux-gnu] 开始 tauri build（bundler: appimage/deb/rpm）..."
npx tauri build --target "$TARGET_TRIPLE"

# ---------- 5. portable tar.gz（含 7z 推荐版 + 单程序 no7z 版） ----------
node scripts/build-portable.mjs --target "$TARGET_TRIPLE" -o "$DIST_DIR"
node scripts/build-portable.mjs --target "$TARGET_TRIPLE" -o "$DIST_DIR" --no-7z

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
    fail "glibc 审计失败：产物依赖 $MAX_GLIBC > 目标 $GLIBC_VERSION（宿主 glibc 过高；请用 build-linux-zig.sh 锁版本，或换 Ubuntu 22.04 构建）"
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
    *.AppImage) dest="nrmm-tauri_${VERSION}_x64-Linux.AppImage" ;;
    *)          dest="$bn" ;;
  esac
  cp "$src" "$DIST_DIR/$dest"
  echo "  [OK] $dest"
}
echo "[build-linux-gnu] 收集并改名产物到 $DIST_DIR/ ..."
rename_to_spec "$(find "$BUNDLE_DIR/deb"      -maxdepth 1 -name '*.deb'      2>/dev/null | head -1)"
rename_to_spec "$(find "$BUNDLE_DIR/rpm"      -maxdepth 1 -name '*.rpm'      2>/dev/null | head -1)"
rename_to_spec "$(find "$BUNDLE_DIR/appimage" -maxdepth 1 -name '*.AppImage' 2>/dev/null | head -1)"
[ -f "$DIST_DIR/latest-linux.json" ] || cp "$(find "$BUNDLE_DIR" -maxdepth 2 -name 'latest.json' 2>/dev/null | head -1)" "$DIST_DIR/latest-linux.json" 2>/dev/null || true

echo ""
echo "===== 构建完成（gnu 轨）====="
ls -la "$DIST_DIR/"
echo "请手动上传以上产物到 Gitee Release（tag: v$VERSION），归入 linux-gnu/ 子目录。"
