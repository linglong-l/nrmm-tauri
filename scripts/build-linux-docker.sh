#!/usr/bin/env bash
#
# build-linux-docker.sh
# 在 Docker 容器中构建 nrmm-rust 的 Linux 安装包 (AppImage/deb/rpm)
# 解决在 Fedora/Arch 等新发行版上 linuxdeploy 无法运行的问题。
#
# 用法:
#   ./scripts/build-linux-docker.sh              # 构建全部 (deb/rpm/appimage)
#   ./scripts/build-linux-docker.sh --appimage   # 仅构建 AppImage
#   ./scripts/build-linux-docker.sh --deb        # 仅构建 deb
#   ./scripts/build-linux-docker.sh --rpm        # 仅构建 rpm
#   ./scripts/build-linux-docker.sh --no-rebuild # 不重新构建 Docker 镜像
#   ./scripts/build-linux-docker.sh --shell      # 进入容器交互 Shell (调试)
#
# 前提: 已安装 Docker 且当前用户在 docker 组中 (或使用 sudo)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
IMAGE_NAME="nrmm-tauri-builder:latest"
DOCKERFILE="$PROJECT_ROOT/docker/Dockerfile"
OUTPUT_DIR="$PROJECT_ROOT/dist-linux"
TARGET="x86_64-unknown-linux-gnu"

REBUILD_IMAGE=true
SHELL_ONLY=false
TARGETS_FLAG=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --appimage) TARGETS_FLAG="appimage"; shift ;;
        --deb)      TARGETS_FLAG="deb"; shift ;;
        --rpm)      TARGETS_FLAG="rpm"; shift ;;
        --no-rebuild) REBUILD_IMAGE=false; shift ;;
        --shell)    SHELL_ONLY=true; shift ;;
        -h|--help)
            grep '^#' "$0" | grep -v '#!/' | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

echo "===== Docker Linux 构建 ====="
echo "项目目录: $PROJECT_ROOT"
echo "输出目录: $OUTPUT_DIR"

DOCKER_CMD="docker"
if ! docker info >/dev/null 2>&1; then
    DOCKER_CMD="sudo docker"
    if ! sudo docker info >/dev/null 2>&1; then
        echo "[FAIL] Docker 不可用, 请先安装并启动 Docker"
        exit 1
    fi
fi

if [ "$REBUILD_IMAGE" = true ]; then
    echo ""
    echo "===== 构建 Docker 镜像 ($IMAGE_NAME) ====="
    USER_ID=$(id -u)
    GROUP_ID=$(id -g)
    $DOCKER_CMD build \
        -t "$IMAGE_NAME" \
        -f "$DOCKERFILE" \
        --build-arg USER_ID="$USER_ID" \
        --build-arg GROUP_ID="$GROUP_ID" \
        "$PROJECT_ROOT/docker"
    echo "[OK] Docker 镜像构建完成"
else
    echo "[SKIP] 使用已有 Docker 镜像 (--no-rebuild)"
fi

if [ "$SHELL_ONLY" = true ]; then
    echo ""
    echo "===== 进入容器 Shell (调试模式) ====="
    $DOCKER_CMD run --rm -it \
        -v "$PROJECT_ROOT:/workspace" \
        -v nrmm-cargo-registry:/usr/local/cargo/registry \
        -v nrmm-cargo-git:/usr/local/cargo/git \
        -v nrmm-rustup:/usr/local/rustup \
        -v nrmm-npm-cache:/home/builder/.npm \
        -u builder \
        -w /workspace \
        "$IMAGE_NAME" \
        /bin/bash
    exit 0
fi

mkdir -p "$OUTPUT_DIR"

echo ""
echo "===== 在容器中构建 Tauri Linux 包 ====="

if [ -n "$TARGETS_FLAG" ]; then
    BUNDLE_ARGS="--bundles $TARGETS_FLAG"
else
    BUNDLE_ARGS="--bundles deb,rpm,appimage"
fi

$DOCKER_CMD run --rm \
    -v "$PROJECT_ROOT:/workspace" \
    -v nrmm-cargo-registry:/usr/local/cargo/registry \
    -v nrmm-cargo-git:/usr/local/cargo/git \
    -v nrmm-rustup:/usr/local/rustup \
    -v nrmm-npm-cache:/home/builder/.npm \
    -u builder \
    -w /workspace \
    -e CARGO_PROFILE_RELEASE_LTO=true \
    -e CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    -e CARGO_PROFILE_RELEASE_STRIP=true \
    "$IMAGE_NAME" \
    /bin/bash -c "
set -e
echo '--- 安装 npm 依赖 ---'
npm ci
echo '--- 前端构建校验 ---'
npm run build
echo '--- Tauri Linux 构建 ---'
npx tauri build --target $TARGET $BUNDLE_ARGS
echo '--- 构建完成 ---'
"

echo ""
echo "===== 收集构建产物 ====="
BUNDLE_DIR="$PROJECT_ROOT/src-tauri/target/$TARGET/release/bundle"

copy_if_exists() {
    local src_dir="$1"
    local pattern="$2"
    local dest_name="$3"
    local found
    found=$(find "$src_dir" -maxdepth 1 -name "$pattern" 2>/dev/null | head -1)
    if [ -n "$found" ]; then
        cp "$found" "$OUTPUT_DIR/$dest_name"
        echo "[OK] $(basename "$found") -> $dest_name"
    else
        echo "[WARN] 未找到 $pattern in $src_dir"
    fi
}

copy_if_exists "$BUNDLE_DIR/appimage" "*.AppImage" "nrmm-rust-x86_64.AppImage"
copy_if_exists "$BUNDLE_DIR/deb" "*.deb" "nrmm-rust-x86_64.deb"
copy_if_exists "$BUNDLE_DIR/rpm" "*.rpm" "nrmm-rust-x86_64.rpm"

LATEST=$(find "$BUNDLE_DIR" -name "latest.json" 2>/dev/null | head -1)
if [ -n "$LATEST" ]; then
    cp "$LATEST" "$OUTPUT_DIR/latest-linux.json"
    echo "[OK] latest-linux.json"
fi

echo ""
echo "===== dist-linux/ 内容 ====="
ls -lh "$OUTPUT_DIR"/
echo ""
echo "===== 构建完成 ====="
echo "产物位于: $OUTPUT_DIR"
