#!/usr/bin/env bash
# ============================================================================
# build-linux-zig.sh — NRMM Linux 手动构建脚本（zig 锁 glibc，Ubuntu 22.04 级兼容）
#
# 用途：
#   在 WSL2 / Ubuntu（含 webkit2gtk-4.1-dev 等系统依赖）中手动构建 Linux 发行包：
#   AppImage / deb / rpm / portable tar.gz，并审计产物 glibc 符号版本。
#
# 设计要点：
#   - zig 通过全局 PATH 引用（不硬编码路径），本机 Windows 侧的
#     D:\Tools\Code\zig-x86_64-dev 与 WSL2 内安装的 zig 均适用（\\wsl$ 下同一脚本）
#   - 原理：CARGO_TARGET_<TRIPLE>_LINKER 指向 "zig cc -target x86_64-linux-gnu.<glibc>"
#     wrapper —— 对 tauri build 内部的 cargo 完全透明，链接全程经 zig，
#     glibc 符号版本锁定（等价 cargo-zigbuild 的核心机制，WSL2 原生编译无交叉 header 问题）
#   - zig 的 lld 默认不产生 DT_RELR（.relr.dyn），从根源规避 linuxdeploy 老 strip
#     无法识别的已知问题；NO_STRIP=1 作为双保险保留
#   - LTO 策略：沿用根 Cargo.toml 现有配置（lto=true）；若日后 AppImage 与 LTO 冲突，
#     再评估拆分编译（export CARGO_PROFILE_RELEASE_LTO=false 即可局部关闭）
#
# 前置依赖（WSL2/Ubuntu 侧）：
#   zig（全局 PATH）、rustup + stable、node >= 22、objdump（binutils）、
#   libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
#   libsoup-3.0-dev libfuse2 patchelf rpm fakeroot
#
# 用法：
#   bash scripts/build-linux-zig.sh                 # 默认 glibc 2.35，输出 dist-linux/
#   GLIBC_VERSION=2.28 bash scripts/build-linux-zig.sh   # 自定义 glibc 下限
# ============================================================================
set -euo pipefail

# ---------- 可配置项（环境变量可覆盖） ----------
TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}"
GLIBC_VERSION="${GLIBC_VERSION:-2.35}"   # Ubuntu 22.04 = glibc 2.35
DIST_DIR="${DIST_DIR:-dist-linux}"
ARCHIVE_MIRROR="${ARCHIVE_MIRROR:-https://www.7-zip.org/a}"

cd "$(dirname "$0")/.."

# ---------- 1. 前置检查 ----------
fail() { echo "[build-linux-zig] ERROR: $*" >&2; exit 1; }

command -v zig   >/dev/null 2>&1 || fail "zig 不在 PATH（请全局安装 zig；Windows 侧目录 D:\\Tools\\Code\\zig-x86_64-dev 需加入 PATH 后经 WSL 调用或直接在 WSL 安装）"
command -v cargo >/dev/null 2>&1 || fail "cargo 不在 PATH"
command -v node  >/dev/null 2>&1 || fail "node 不在 PATH（需 >= 22）"
command -v objdump >/dev/null 2>&1 || fail "objdump 不在 PATH（apt install binutils）"

ZIG_VERSION="$(zig version)"
echo "[build-linux-zig] zig $ZIG_VERSION | target=$TARGET_TRIPLE | glibc<=$GLIBC_VERSION | 输出=$DIST_DIR"

rustup target list --installed | grep -q "^$TARGET_TRIPLE" || {
  echo "[build-linux-zig] 安装 Rust target: $TARGET_TRIPLE"
  rustup target add "$TARGET_TRIPLE"
}

# ---------- 2. zig cc wrapper（锁 glibc 符号版本，对 cargo/tauri 透明） ----------
WRAPPER_DIR="$(mktemp -d)"
trap 'rm -rf "$WRAPPER_DIR"' EXIT
ZIG_ARCH="${TARGET_TRIPLE%%-*}"          # x86_64 / i686 / aarch64...
ZIG_CC="$WRAPPER_DIR/zig-cc-glibc"
cat > "$ZIG_CC" <<EOF
#!/usr/bin/env bash
# 由 build-linux-zig.sh 生成：以指定 glibc 符号版本调用 zig cc 作为 linker
exec zig cc -target ${ZIG_ARCH}-linux-gnu.${GLIBC_VERSION} "\$@"
EOF
chmod +x "$ZIG_CC"
# CARGO_TARGET_<TRIPLE>_LINKER 的变量名需按 triple 大写化（- -> _）。若此前未导出（默认 undefined），
# 由这里导出，tauri/cargo 即会经 zig 链接（trigger 一次即可贯穿整个构建）。
TRIPLE_ENV="$(echo "$TARGET_TRIPLE" | tr 'a-z' 'A-Z' | tr '-' '_')"
export "CARGO_TARGET_${TRIPLE_ENV}_LINKER=$ZIG_CC"

# AppImage 构建兜底（FUSE 不可用环境解压运行 AppImage 工具；跳过 linuxdeploy 内置 strip）
export APPIMAGE_EXTRACT_AND_RUN=1
export NO_STRIP=1

# ---------- 3. 7z Linux sidecar 检查/下载（目录被 gitignore，新检出环境需自补） ----------
# 按目标架构选择官方 tar.xz 并 staging 到 current/（tauri.linux.conf.json resources 指向
# 7z2602-linux-current/，7zz/7zzs 须为匹配架构版本）。注意：官方 32 位包名为 x86（非 x32/i686）。
Z7_ARCH_DIR="src-tauri/binaries/7z/7z2602-linux-current"
case "$ZIG_ARCH" in
  x86_64) SEVENZIP_TAR="7z2602-linux-x64.tar.xz"  ;;
  i686)   SEVENZIP_TAR="7z2602-linux-x86.tar.xz"  ;;
  *) fail "不支持的架构: $ZIG_ARCH（7z Linux sidecar 仅提供 x86_64 / i686）" ;;
esac
SEVENZIP_SRC="src-tauri/binaries/7z/7z2602-linux-$ZIG_ARCH"
if [ ! -f "$SEVENZIP_SRC/7zz" ] || [ ! -f "$SEVENZIP_SRC/7zzs" ]; then
  echo "[build-linux-zig] 下载 7z Linux sidecar ($SEVENZIP_TAR)..."
  mkdir -p "$SEVENZIP_SRC"
  curl -fSL --retry 3 -o "/tmp/$SEVENZIP_TAR" "$ARCHIVE_MIRROR/$SEVENZIP_TAR" \
    || fail "下载 $SEVENZIP_TAR 失败（可用 ARCHIVE_MIRROR 环境变量换镜像），或手动放置该包并解压后将其 7zz/7zzs 放到 $SEVENZIP_SRC/ 重试"
  tar -xJf "/tmp/$SEVENZIP_TAR" -C "$SEVENZIP_SRC"
  rm -f "/tmp/$SEVENZIP_TAR"
  # 官方包结构各版本可能把 7zz/7zzs 放在顶层或子目录，这里统一定位并提升到 $SEVENZIP_SRC 顶层
  for f in 7zz 7zzs; do
    SRC_FILE="$(find "$SEVENZIP_SRC" -type f -name "$f" | head -1 || true)"
    [ -n "$SRC_FILE" ] && cp "$SRC_FILE" "$SEVENZIP_SRC/$f"
  done
fi
[ -f "$SEVENZIP_SRC/7zz" ] && [ -f "$SEVENZIP_SRC/7zzs" ] \
  || fail "7z sidecar 不完整：$SEVENZIP_SRC/7zz、7zzs 缺失"

# staging 到 current/：先清空再复制匹配架构的 7zz/7zzs
rm -rf "$Z7_ARCH_DIR"
mkdir -p "$Z7_ARCH_DIR"
cp "$SEVENZIP_SRC/7zz"  "$Z7_ARCH_DIR/7zz"
cp "$SEVENZIP_SRC/7zzs" "$Z7_ARCH_DIR/7zzs"
echo "[build-linux-zig] 7z sidecar staged: $ZIG_ARCH -> current (target=$TARGET_TRIPLE)"

# ---------- 4. 前端依赖 ----------
if [ ! -d node_modules ]; then
  echo "[build-linux-zig] 安装 npm 依赖..."
  npm ci
fi

# ---------- 5. Tauri 构建（AppImage/deb/rpm，链接全程经 zig） ----------
# 注意：不得用 `npm run tauri build -- --target X`——npm 会把 `--target` 当本身配置吞掉
# （Unknown cli config --target），仅将三连作为位置参数传给 tauri，cargo 会报
# "unexpected argument"。统一用 npx tauri 直传（2026-08-18 实测 Windows 侧确认，bash 同理）。
echo "[build-linux-zig] 开始 tauri build（bundler: appimage/deb/rpm）..."
npx tauri build --target "$TARGET_TRIPLE"

# ---------- 6. portable tar.gz（复用恢复的 build-portable.mjs） ----------
node scripts/build-portable.mjs --target "$TARGET_TRIPLE" -o "$DIST_DIR"

# ---------- 7. glibc 符号版本审计（防 linker 静默回退到系统 gcc） ----------
# 探测 release 目录（workspace 根 target 优先，回退 src-tauri/target）
detect_release_dir() {
  for d in "target/$TARGET_TRIPLE/release" "src-tauri/target/$TARGET_TRIPLE/release"; do
    [ -d "$d" ] && { echo "$d"; return; }
  done
  fail "找不到 release 目录（target/ 与 src-tauri/target/ 均无）"
}
RELEASE_DIR="$(detect_release_dir)"
BIN="$RELEASE_DIR/nrmm-rust"
[ -f "$BIN" ] || fail "主二进制不存在: $BIN"

MAX_GLIBC="$(objdump -T "$BIN" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -uV | tail -1 || true)"
echo "[build-linux-zig] 产物最大 glibc 符号版本: ${MAX_GLIBC:-无（静态/无符号依赖）}"
if [ -n "$MAX_GLIBC" ]; then
  NEWER="$(printf '%s\n%s\n' "$GLIBC_VERSION" "$MAX_GLIBC" | sort -V | tail -1)"
  if [ "$NEWER" != "$GLIBC_VERSION" ]; then
    fail "glibc 审计失败：产物依赖 $MAX_GLIBC > 目标 $GLIBC_VERSION（linker 可能回退到了系统 gcc，检查 CARGO_TARGET_*_LINKER 设置）"
  fi
fi
echo "[build-linux-zig] glibc 审计通过（<= $GLIBC_VERSION）"

# ---------- 8. 汇总产物 ----------
BUNDLE_DIR="$RELEASE_DIR/bundle"
mkdir -p "$DIST_DIR"
copy_first_found() {  # copy_first_found <glob...> <dest>
  local dest="$1"; shift
  for pat in "$@"; do
    local src
    src="$(find $pat -maxdepth 1 2>/dev/null | head -1 || true)"
    if [ -n "$src" ] && [ -f "$src" ]; then
      cp "$src" "$DIST_DIR/$dest"
      echo "  [OK] $dest"
      return
    fi
  done
  echo "  [WARN] 未找到: $dest"
}
echo "[build-linux-zig] 收集产物到 $DIST_DIR/ ..."
copy_first_found "nrmm-rust-x86_64.AppImage" "$BUNDLE_DIR/appimage/*.AppImage"
copy_first_found "nrmm-rust-x86_64.deb"      "$BUNDLE_DIR/deb/*.deb"
copy_first_found "nrmm-rust-x86_64.rpm"      "$BUNDLE_DIR/rpm/*.rpm"
# portable 已由 build-portable.mjs 直接输出；updater manifest（如有）
copy_first_found "latest-linux.json" "$BUNDLE_DIR/*/latest.json" "$BUNDLE_DIR/latest.json"

echo ""
echo "===== 构建完成 ====="
ls -la "$DIST_DIR/"
echo "请手动上传以上产物到 Gitee Release（tag: v*.*.*），latest-linux.json 用于 updater。"
