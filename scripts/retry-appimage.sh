#!/usr/bin/env bash
# retry-appimage.sh - 带正确 env + verbose 的 appimage 单目标重跑脚本（在 WSL 里执行）
set -o pipefail

# ===== 最保险：手动补 PATH 覆盖 3 种常见 WSL2/Fedora 安装方式 =====
# 1) Rust cargo/rustup
# [ -d "$HOME/.cargo/bin" ] && export PATH="$HOME/.cargo/bin:$PATH"
# 2) Node.js 自定义安装路径（本项目实际使用的 /usr/local/DevelopEnv/nodejs/bin）
# [ -d "/usr/local/DevelopEnv/nodejs/bin" ] && export PATH="/usr/local/DevelopEnv/nodejs/bin:$PATH"
# 3) Node.js 常规 /usr/local/bin / nvm ~/.nvm 兜底
# [ -s "$HOME/.nvm/nvm.sh" ] && . "$HOME/.nvm/nvm.sh"
# export PATH="/usr/local/bin:/usr/bin:/usr/sbin:$PATH"

PROJ="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"/..
cd "$PROJ" || exit 2

LOG="$PROJ/build-appimage-verbose.log"
# ===== 设置 4 条核心环境变量（最关键！） =====
export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH=x86_64
export NO_STRIP=1
export STRIP="$(command -v strip || echo /usr/sbin/strip)"

echo "===== [ENV/PATH] 已就绪 ====="
echo "  PATH (前5条)       : $(echo "$PATH" | cut -d: -f1-5)"
echo "  APPIMAGE_EXTRACT..= $APPIMAGE_EXTRACT_AND_RUN"
echo "  ARCH               = $ARCH"
echo "  NO_STRIP           = $NO_STRIP"
echo "  STRIP              = $STRIP"
echo "  glibc ldd          = $(ldd --version | head -1)"
echo "  node               = $(node --version 2>&1)"
echo "  npm                = $(npm --version 2>&1)"
echo "  npx                = $(npx --version 2>&1)"
echo "  cargo              = $(cargo --version 2>&1)"
echo "  linuxdeploy cache dir:"
ls -la ~/.cache/tauri/linuxdeploy-* 2>&1 | sed "s/^/    /"
echo ""
echo "===== [BUILD] npx tauri build -v --target x86_64-unknown-linux-gnu --bundles appimage  ====="
echo "  日志: $LOG"
echo ""

# 清理上次失败的 AppDir 残留，避免脏缓存影响
rm -rf src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/appimage

set +e
(
  npx tauri build -v \
    --target x86_64-unknown-linux-gnu \
    --bundles appimage
) 2>&1 | tee "$LOG"
TAURI_EXIT=${PIPESTATUS[0]}
set -e

echo ""
echo "===== [RESULT] tauri exit code = $TAURI_EXIT  ====="
shopt -s nullglob
APPIMG=( src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/appimage/*.AppImage )
if [ ${#APPIMG[@]} -gt 0 ]; then
  echo "✅ AppImage 构建成功:"
  ls -lah "${APPIMG[@]}"
  exit 0
else
  echo "❌ AppImage 仍然失败。查看详细日志: $LOG"
  echo "  关键报错最后 30 行:"
  tail -30 "$LOG"
  exit "$TAURI_EXIT"
fi
