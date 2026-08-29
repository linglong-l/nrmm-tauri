#!/usr/bin/env bash
# ============================================================================
# build-linux-zig.sh — NRMM Linux 手动构建脚本（zig 锁 glibc，Ubuntu 22.04 级兼容）
#
# 用途：
#   在 WSL2 / Ubuntu（含 webkit2gtk-4.1-dev 等系统依赖）中手动构建 Linux 发行包：
#   AppImage / deb / rpm / portable tar.gz（含 7z 与单程序 no7z 两版），
#   并审计产物 glibc 符号版本。
#
# 设计要点：
#   - 仅支持 x86_64（项目约束：不做交叉编译、不兼容 x86_32/x32）。
#   - 内置 7z 使用动态链接的 7zz（实测 GLIBC 最高 2.16、CXXABI 1.3.9，远低于
#     项目下限 glibc 2.35）；如需静态版 7zzs，使用者自行修改本脚本与
#     tauri.linux.conf.json 的 resources 映射。
#   - zig 通过全局 PATH 引用（不硬编码路径），本机 Windows 侧的
#     D:\Tools\Code\zig-x86_64-dev 与 WSL2 内安装的 zig 均适用（\\wsl$ 下同一脚本）
#   - 原理：CARGO_TARGET_<TRIPLE>_LINKER / CC_<TRIPLE> / CXX_<TRIPLE> 均指向
#     "zig cc/c++ -target x86_64-linux-gnu.<glibc>" wrapper —— 对 tauri build 内部的
#     cargo 完全透明，链接与 vendored C++ 编译全程经 zig，glibc 符号版本锁定
#     （等价 cargo-zigbuild 的核心机制，WSL2 原生编译无交叉 header 问题）
#   - zig 的 lld 默认不产生 DT_RELR（.relr.dyn），从根源规避 linuxdeploy 老 strip
#     无法识别的已知问题；NO_STRIP=1 作为双保险保留
#   - 输出归入 dist-linux/zig/；产物文件名已改写为 nrmm-tauri_<版本>_x64-Linux.* 规范。
#
# 前置依赖（WSL2/Ubuntu 侧）：
#   zig（全局 PATH）、rustup + stable、node >= 22、objdump（binutils）、
#   libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
#   libsoup-3.0-dev libfuse2 patchelf rpm fakeroot
#   以及 build-essential（提供 g++ / libstdc++-dev / libgcc）—— 必须！unrar 是 C++，
#   zig 的 lld 通过 -lstdc++ 链接 libstdc++.so（dev 软链接）；缺失时链接阶段会报
#   "undefined symbol: std::__cxx11::basic_string*" 与 "__cpu_model"。
#   全新环境若未安装：sudo apt-get install -y build-essential
#
# 用法：
#   bash scripts/build-linux-zig.sh                 # 默认 glibc 2.35，输出 dist-linux/zig/
#   GLIBC_VERSION=2.28 bash scripts/build-linux-zig.sh   # 自定义 glibc 下限
# ============================================================================
set -euo pipefail

# ---------- 可配置项（环境变量可覆盖） ----------
TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}"
GLIBC_VERSION="${GLIBC_VERSION:-2.35}"   # Ubuntu 22.04 = glibc 2.35
DIST_DIR="${DIST_DIR:-dist-linux/zig}"
ARCHIVE_MIRROR="${ARCHIVE_MIRROR:-https://www.7-zip.org/a}"

cd "$(dirname "$0")/.."

# ---------- 0. 版本号（命名规范根基，与 package.json 一致） ----------
VERSION="$(node -p "require('./package.json').version" 2>/dev/null || echo "0.0.0")"

# ---------- 1. 前置检查 ----------
fail() { echo "[build-linux-zig] ERROR: $*" >&2; exit 1; }

command -v zig   >/dev/null 2>&1 || fail "zig 不在 PATH（请全局安装 zig；Windows 侧目录 D:\\Tools\\Code\\zig-x86_64-dev 需加入 PATH 后经 WSL 调用或直接在 WSL 安装）"
command -v cargo >/dev/null 2>&1 || fail "cargo 不在 PATH"
command -v node  >/dev/null 2>&1 || fail "node 不在 PATH（需 >= 22）"
command -v objdump >/dev/null 2>&1 || fail "objdump 不在 PATH（apt install binutils）"

# 仅支持 x86_64
case "$TARGET_TRIPLE" in
  x86_64-*) ;;
  *) fail "不支持的架构: $TARGET_TRIPLE（项目约束：仅 x86_64，不做交叉编译）" ;;
esac

ZIG_VERSION="$(zig version)"
echo "[build-linux-zig] zig $ZIG_VERSION | target=$TARGET_TRIPLE | glibc<=$GLIBC_VERSION | 输出=$DIST_DIR | 版本=$VERSION"

rustup target list --installed | grep -q "^$TARGET_TRIPLE" || {
  echo "[build-linux-zig] 安装 Rust target: $TARGET_TRIPLE"
  rustup target add "$TARGET_TRIPLE"
}

# ---------- 2. zig cc / c++ wrapper（锁 glibc 符号版本，对 cargo/tauri 透明） ----------
WRAPPER_DIR="$(mktemp -d)"
trap 'rm -rf "$WRAPPER_DIR"' EXIT
ZIG_CC="$WRAPPER_DIR/zig-cc-glibc"
ZIG_CXX="$WRAPPER_DIR/zig-cxx-glibc"
cat > "$ZIG_CC" <<EOF
#!/usr/bin/env bash
# 由 build-linux-zig.sh 生成：以指定 glibc 符号版本调用 zig cc 作为 linker / C 编译器
exec zig cc -target x86_64-linux-gnu.${GLIBC_VERSION} "\$@"
EOF
cat > "$ZIG_CXX" <<EOF
#!/usr/bin/env bash
# 由 build-linux-zig.sh 生成：以指定 glibc 符号版本调用 zig c++ 编译 vendored C++（unrar 等）
exec zig c++ -target x86_64-linux-gnu.${GLIBC_VERSION} "\$@"
EOF
chmod +x "$ZIG_CC" "$ZIG_CXX"
# CARGO_TARGET_<TRIPLE>_LINKER 的变量名需按 triple 大写化（- -> _）。若此前未导出（默认 undefined），
# 由这里导出，tauri/cargo 即会经 zig 链接（trigger 一次即可贯穿整个构建）。
# 同时导出 CC_<TRIPLE> / CXX_<TRIPLE>：unrar-sys 的 vendored UnRAR C++ 经 cc crate 调用，
# 仅设 LINKER 会令其回退到 host g++（24.04 → glibc 2.39 头文件），逃逸 glibc 锁定。
TRIPLE_ENV="$(echo "$TARGET_TRIPLE" | tr 'a-z' 'A-Z' | tr '-' '_')"
export "CARGO_TARGET_${TRIPLE_ENV}_LINKER=$ZIG_CC"
export "CC_${TRIPLE_ENV}=$ZIG_CC"
export "CXX_${TRIPLE_ENV}=$ZIG_CXX"

# AppImage 构建兜底（FUSE 不可用环境解压运行 AppImage 工具；跳过 linuxdeploy 内置 strip）
export APPIMAGE_EXTRACT_AND_RUN=1
export NO_STRIP=1

# ---------- 3. 7z Linux sidecar 检查/下载（目录被 gitignore，新检出环境需自补） ----------
# 仅 x86_64 + 动态 7zz（tauri.linux.conf.json resources 指向 7z2602-linux-x64/7zz）。
# 如需静态 7zzs，使用者自行修改本段与 conf 的 resources 映射。
SEVENZIP_TAR="7z2602-linux-x64.tar.xz"
SEVENZIP_SRC="src-tauri/binaries/7z/7z2602-linux-x64"
if [ ! -f "$SEVENZIP_SRC/7zz" ]; then
  echo "[build-linux-zig] 下载 7z Linux sidecar ($SEVENZIP_TAR)..."
  mkdir -p "$SEVENZIP_SRC"
  curl -fSL --retry 3 -o "/tmp/$SEVENZIP_TAR" "$ARCHIVE_MIRROR/$SEVENZIP_TAR" \
    || fail "下载 $SEVENZIP_TAR 失败（可用 ARCHIVE_MIRROR 环境变量换镜像），或手动放置该包并解压后将其 7zz 放到 $SEVENZIP_SRC/ 重试"
  tar -xJf "/tmp/$SEVENZIP_TAR" -C "$SEVENZIP_SRC"
  rm -f "/tmp/$SEVENZIP_TAR"
  # 官方包结构可能把 7zz 放在顶层或子目录，统一定位并提升到 $SEVENZIP_SRC 顶层
  SRC_FILE="$(find "$SEVENZIP_SRC" -type f -name '7zz' | head -1 || true)"
  [ -n "$SRC_FILE" ] && cp "$SRC_FILE" "$SEVENZIP_SRC/7zz"
fi
[ -f "$SEVENZIP_SRC/7zz" ] || fail "7z sidecar 不完整：$SEVENZIP_SRC/7zz 缺失"
echo "[build-linux-zig] 7z sidecar ready: $SEVENZIP_SRC/7zz (x86_64, 动态 7zz)"

# ---------- 4. 前端依赖 ----------
if [ ! -d node_modules ]; then
  echo "[build-linux-zig] 安装 npm 依赖..."
  npm ci
fi

# ---------- 5. Tauri 构建（AppImage/deb/rpm，编译全程经 zig） ----------
# 注意：不得用 `npm run tauri build -- --target X`——npm 会把 `--target` 当本身配置吞掉
# （Unknown cli config --target），仅将三连作为位置参数传给 tauri，cargo 会报
# "unexpected argument"。统一用 npx tauri 直传（2026-08-18 实测 Windows 侧确认，bash 同理）。
echo "[build-linux-zig] 开始 tauri build（bundler: appimage/deb/rpm）..."
npx tauri build --target "$TARGET_TRIPLE"

# ---------- 6. portable tar.gz（含 7z 推荐版 + 单程序 no7z 版） ----------
node scripts/build-portable.mjs --target "$TARGET_TRIPLE" -o "$DIST_DIR"
node scripts/build-portable.mjs --target "$TARGET_TRIPLE" -o "$DIST_DIR" --no-7z

# ---------- 7. glibc 符号版本审计（防 linker 静默回退到系统 gcc） ----------
# 探测 release 目录（workspace 根 target 优先，回退 src-tauri/target）
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
echo "[build-linux-zig] 产物最大 glibc 符号版本: ${MAX_GLIBC:-无（静态/无符号依赖）}"
if [ -n "$MAX_GLIBC" ]; then
  NEWER="$(printf '%s\n%s\n' "$GLIBC_VERSION" "$MAX_GLIBC" | sort -V | tail -1)"
  if [ "$NEWER" != "$GLIBC_VERSION" ]; then
    fail "glibc 审计失败：产物依赖 $MAX_GLIBC > 目标 $GLIBC_VERSION（linker 可能回退到了系统 gcc，检查 CARGO_TARGET_*_LINKER / CC_* / CXX_* 设置）"
  fi
fi
echo "[build-linux-zig] glibc 审计通过（<= $GLIBC_VERSION）"

# ---------- 8. 汇总产物（bundler 出厂名 -> 规范名） ----------
BUNDLE_DIR="$RELEASE_DIR/bundle"
mkdir -p "$DIST_DIR"
rename_to_spec() {  # rename_to_spec <src> <factory-ext>
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
echo "[build-linux-zig] 收集并改名产物到 $DIST_DIR/ ..."
rename_to_spec "$(find "$BUNDLE_DIR/deb"      -maxdepth 1 -name '*.deb'      2>/dev/null | head -1)"
rename_to_spec "$(find "$BUNDLE_DIR/rpm"      -maxdepth 1 -name '*.rpm'      2>/dev/null | head -1)"
rename_to_spec "$(find "$BUNDLE_DIR/appimage" -maxdepth 1 -name '*.AppImage' 2>/dev/null | head -1)"
# portable tar.gz 已由 build-portable.mjs 直接输出规范名；updater manifest（如有）
[ -f "$DIST_DIR/latest-linux.json" ] || cp "$(find "$BUNDLE_DIR" -maxdepth 2 -name 'latest.json' 2>/dev/null | head -1)" "$DIST_DIR/latest-linux.json" 2>/dev/null || true

echo ""
echo "===== 构建完成 ====="
ls -la "$DIST_DIR/"
echo "请手动上传以上产物到 Gitee Release（tag: v$VERSION）。"
echo "gnu 轨产物请归入 Release 的 linux-gnu/ 子目录，本 zig 轨归入 linux-zig/ 子目录（同名不冲突）。"
