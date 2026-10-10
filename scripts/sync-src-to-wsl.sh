#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# sync-src-to-wsl.sh — 在 WSL2 内执行：把 Windows 工作区的构建输入同步到 WSL2 构建树
#
# 背景：Windows 工作区是唯一真源，但存在「已改未提交」的文件。WSL2 侧若只做
#       git pull，拿到的只是已提交内容 -> 编译报 E0433 / E0609（模块或字段缺失）。
#       本脚本按文件直拷（不走 git），把工作区当前状态完整搬到 WSL2。
#
# 用法（在 WSL2 内）：
#   ./scripts/sync-src-to-wsl.sh                       # 默认目标 ~/Develop/Projects/Rust/XXMI-NRMM
#   ./scripts/sync-src-to-wsl.sh /path/to/build-tree   # 指定目标
#   SRC=/mnt/d/other/path ./scripts/sync-src-to-wsl.sh # 指定源（默认 /mnt/d/PC/TestProjects/XXMI-NRMM）
#
# 说明：
#   - 故意不同步 src-tauri/Cargo.lock：仓库内该文件是 nrmm-rust 0.3.0 遗留的过期锁，
#     唯一有效的是仓库根目录 Cargo.lock，拷过去会污染 Linux 构建。
#   - 排除 target/（构建产物，跨平台不可复用）与 gen/（build.rs 会重新生成）。
#   - 会同步 .cargo/：平台差异化 rustflags 定义在 .cargo/config.toml，缺了它 Linux 构建
#     会静默丢掉 --as-needed / relro / now 链接加固（cargo 按当前目录向上查找该文件）。
# ---------------------------------------------------------------------------
set -euo pipefail

SRC="${SRC:-/mnt/d/PC/TestProjects/XXMI-NRMM}"
DST="${1:-$HOME/Develop/Projects/Rust/XXMI-NRMM}"

if [ ! -d "$SRC/src-tauri" ]; then
    echo "ERROR: 源目录不存在或不是项目根：$SRC" >&2
    echo "       用 SRC=... 指定正确的挂载路径" >&2
    exit 1
fi
mkdir -p "$DST"

RSYNC_FILTER=(
    --exclude 'target/'
    --exclude 'gen/'
    --exclude 'Cargo.lock'
)

if command -v rsync >/dev/null 2>&1; then
    RSYNC_OPTS=(-a --delete "${RSYNC_FILTER[@]}")
    echo "==> rsync: $SRC/src-tauri/ -> $DST/src-tauri/"
    rsync "${RSYNC_OPTS[@]}" "$SRC/src-tauri/" "$DST/src-tauri/"

    echo "==> rsync: 根目录 Cargo.toml / Cargo.lock"
    rsync -a "$SRC/Cargo.toml" "$DST/Cargo.toml"
    rsync -a "$SRC/Cargo.lock" "$DST/Cargo.lock"

    # .cargo/ 必须一起拷：平台差异化 rustflags（--as-needed / relro / now）定义在
    # .cargo/config.toml，cargo 从当前目录向上找该文件。漏掉它不会报错，只会让 Linux
    # 构建树静默丢掉这几条链接加固，产物与真实构建口径不一致。
    if [ -d "$SRC/.cargo" ]; then
        echo "==> rsync: .cargo/（平台 rustflags）"
        rsync -a --delete "$SRC/.cargo/" "$DST/.cargo/"
    fi

    echo "==> rsync: 前端构建输入（src/ package.json vite.config.ts 等）"
    for f in src package.json package-lock.json vite.config.ts vitest.config.ts tsconfig.json tsconfig.node.json index.html; do
        [ -e "$SRC/$f" ] || continue
        if [ -d "$SRC/$f" ]; then
            rsync -a --delete "$SRC/$f/" "$DST/$f/"
        else
            rsync -a "$SRC/$f" "$DST/$f"
        fi
    done
else
    echo "WARN: 未找到 rsync，退化为 cp（不会删除目标端多余文件，请手动清理）" >&2
    mkdir -p "$DST/src-tauri"
    for item in binaries build.rs capabilities icons src tests \
                Cargo.toml tauri.conf.json tauri.linux.conf.json tauri.windows.conf.json; do
        [ -e "$SRC/src-tauri/$item" ] || continue
        cp -ru "$SRC/src-tauri/$item" "$DST/src-tauri/"
    done
    cp -u "$SRC/Cargo.toml" "$DST/Cargo.toml"
    cp -u "$SRC/Cargo.lock" "$DST/Cargo.lock"
    if [ -d "$SRC/.cargo" ]; then
        mkdir -p "$DST/.cargo"
        cp -u "$SRC/.cargo/"* "$DST/.cargo/"
    fi
fi

# ---------------------------------------------------------------------------
# 一致性校验：确认新增模块与新字段确实到位（否则构建仍会以同样的 E0433/E0609 失败）
# ---------------------------------------------------------------------------
echo
echo "==> 校验关键文件一致性"
fail=0
check() {
    if eval "$2"; then
        echo "  [OK]   $1"
    else
        echo "  [FAIL] $1"
        fail=1
    fi
}

check "core/fs_walk.rs 已同步" \
    "test -f \"$DST/src-tauri/src/core/fs_walk.rs\""
check "core/mod.rs 含 'pub mod fs_walk;'" \
    "grep -q 'pub mod fs_walk;' \"$DST/src-tauri/src/core/mod.rs\""
check "扫描域含 invalid_utf8_count 字段（不锁定文件名，拆分后不再失效）" \
    "grep -rq 'pub invalid_utf8_count' \"$DST/src-tauri/src/core/\""
check ".cargo/config.toml 已同步（Linux 侧链接加固依赖它）" \
    "test -f \"$DST/.cargo/config.toml\""
check "models/mod_data.rs 已同步到最新版本" \
    "cmp -s \"$SRC/src-tauri/src/models/mod_data.rs\" \"$DST/src-tauri/src/models/mod_data.rs\""
check "src-tauri/Cargo.lock 未被污染（不随同步拷贝）" \
    "! cmp -s \"$SRC/src-tauri/Cargo.lock\" \"$DST/src-tauri/Cargo.lock\" || ! test -f \"$DST/src-tauri/Cargo.lock\""

echo
if [ "$fail" -eq 0 ]; then
    echo "同步完成，可以开始构建："
    echo "  cd \"$DST\" && cargo build --target x86_64-unknown-linux-gnu --release"
else
    echo "同步后仍有不一致项，请先处理上述 [FAIL] 再构建。" >&2
    exit 1
fi
