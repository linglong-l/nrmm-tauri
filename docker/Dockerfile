# ===== 基础镜像：Ubuntu 24.04 (noble) =====
# Ubuntu 24.04 改用 deb822 格式的 sources.list.d/ubuntu.sources
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
ENV LANG=C.UTF-8
ENV LC_ALL=C.UTF-8

# ===== 1. APT 源：尝试阿里云镜像，不可达则回退到官方源 =====
# - Ubuntu 24.04 的 deb822 源文件是 /etc/apt/sources.list.d/ubuntu.sources
# - 22.04/更早用 /etc/apt/sources.list
# - 通过 getent + HTTP 双重检测 mirrors.aliyun.com 是否可达；不可达就不换源，避免 DNS 解析失败造成 apt 空库
RUN set -eux; \
    APT_REACHABLE=0; \
    if command -v getent >/dev/null 2>&1 && getent hosts mirrors.aliyun.com >/dev/null 2>&1; then \
        APT_REACHABLE=1; \
    elif (apt-get update -o Acquire::Retries=1 >/dev/null 2>&1) && \
         (curl -fsSL --max-time 5 http://mirrors.aliyun.com/ubuntu/ >/dev/null 2>&1 || \
          wget -q --timeout=5 -O /dev/null http://mirrors.aliyun.com/ubuntu/ >/dev/null 2>&1); then \
        APT_REACHABLE=1; \
    fi; \
    if [ "$APT_REACHABLE" = "1" ]; then \
        echo "[APT] 使用阿里云镜像 mirrors.aliyun.com"; \
        if [ -f /etc/apt/sources.list.d/ubuntu.sources ]; then \
            sed -i 's|http://archive.ubuntu.com/ubuntu|http://mirrors.aliyun.com/ubuntu|g' /etc/apt/sources.list.d/ubuntu.sources; \
            sed -i 's|http://security.ubuntu.com/ubuntu|http://mirrors.aliyun.com/ubuntu|g' /etc/apt/sources.list.d/ubuntu.sources; \
        else \
            sed -i 's/archive.ubuntu.com/mirrors.aliyun.com/g' /etc/apt/sources.list; \
            sed -i 's/security.ubuntu.com/mirrors.aliyun.com/g' /etc/apt/sources.list; \
        fi; \
    else \
        echo "[APT][WARN] 阿里云镜像不可达，使用官方 archive.ubuntu.com 默认源"; \
    fi; \
    apt-get update -o Acquire::Retries=3

# ===== 2. 安装系统依赖 =====
# Tauri v2 WebKit: Ubuntu 24.04 同时提供 libwebkit2gtk-4.1-dev 和 4.0-dev，优先 4.1；
# 如果目标镜像缺失则回退到 4.0 或 6.0（webextension-gtk）。
# 追加 apt-get install -f -y 修复任何依赖破损
RUN set -eux; \
    if apt-get install -y --no-install-recommends libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev; then \
        echo "[APT] 使用 libwebkit2gtk-4.1-dev"; \
    elif apt-get install -y --no-install-recommends libwebkit2gtk-4.0-dev libjavascriptcoregtk-4.0-dev; then \
        echo "[APT] 回退到 libwebkit2gtk-4.0-dev"; \
    else \
        echo "[APT] 回退到 libwebkitgtk-6.0-dev"; \
        apt-get install -y --no-install-recommends libwebkitgtk-6.0-dev libjavascriptcoregtk-6.0-dev; \
    fi; \
    apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        pkg-config \
        curl \
        wget \
        file \
        git \
        ca-certificates \
        libgtk-3-dev \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        patchelf \
        libssl-dev \
        libxdo-dev \
        libcurl4-openssl-dev \
        libfuse2 \
        rpm \
        fakeroot \
        dpkg-dev \
        sudo \
        xz-utils \
    ; \
    apt-get install -f -y; \
    rm -rf /var/lib/apt/lists/*

# ===== 3. 安装 Node.js 20 + 配置 npm 镜像 =====
# 优先级 1: deb.nodesource.com 脚本（官方）
# 优先级 2: 淘宝 npmmirror 预编译 tarball（https://registry.npmmirror.com/-/binary/node/）
# 优先级 3: nodejs.org 官方 dist tarball（兜底）
RUN set -eux; \
    NODE_VERSION=v20.19.0; \
    INSTALLED=0; \
    if curl -fsSL --max-time 30 https://deb.nodesource.com/setup_20.x | bash -; then \
        apt-get update -o Acquire::Retries=3 && apt-get install -y --no-install-recommends nodejs && INSTALLED=1; \
    fi; \
    if [ "$INSTALLED" != "1" ]; then \
        for URL in \
            "https://registry.npmmirror.com/-/binary/node/${NODE_VERSION}/node-${NODE_VERSION}-linux-x64.tar.xz" \
            "https://nodejs.org/dist/${NODE_VERSION}/node-${NODE_VERSION}-linux-x64.tar.xz" \
        ; do \
            echo "[Node.js] 尝试: $URL"; \
            if curl -fsSL --max-time 120 -o /tmp/node.tar.xz "$URL"; then \
                tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1; \
                rm -f /tmp/node.tar.xz; \
                INSTALLED=1; \
                break; \
            fi; \
        done; \
    fi; \
    if [ "$INSTALLED" != "1" ]; then echo "[Node.js] ERROR: 所有下载源均失败"; exit 1; fi; \
    rm -rf /var/lib/apt/lists/*; \
    node -v; npm -v; \
    if curl -fsSL --max-time 10 https://registry.npmmirror.com >/dev/null 2>&1; then \
        npm config set registry https://registry.npmmirror.com; \
        echo "[npm] 使用淘宝 npm 镜像"; \
    else \
        echo "[npm][WARN] npmmirror 不可达，使用默认 registry.npmjs.org"; \
    fi; \
    npm install -g @tauri-apps/cli@2

ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH

# ===== 4. 安装 Rust (rustup) =====
# - 优先阿里云 rustup 镜像；失败则回退到官方 static.rust-lang.org
RUN set -eux; \
    if curl -fsSL --max-time 10 https://mirrors.aliyun.com/rustup/ >/dev/null 2>&1; then \
        export RUSTUP_DIST_SERVER=https://mirrors.aliyun.com/rustup; \
        export RUSTUP_UPDATE_ROOT=https://mirrors.aliyun.com/rustup/rustup; \
        echo "[Rust] 使用阿里云 rustup 镜像"; \
    else \
        echo "[Rust][WARN] 阿里云 rustup 镜像不可达，使用官方 static.rust-lang.org"; \
    fi; \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable; \
    rustup target add x86_64-unknown-linux-gnu; \
    chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# ===== 5. Cargo crates 镜像源（仅当镜像可达时启用替换） =====
RUN set -eux; \
    mkdir -p "$CARGO_HOME"; \
    CONFIG="$CARGO_HOME/config.toml"; \
    touch "$CONFIG"; \
    if curl -fsSL --max-time 10 "https://mirrors.ustc.edu.cn/crates.io-index/config.json" >/dev/null 2>&1; then \
        { \
            echo '[source.crates-io]'; \
            echo 'replace-with = "ustc"'; \
            echo ''; \
            echo '[source.ustc]'; \
            echo 'registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"'; \
        } > "$CONFIG"; \
        echo "[Cargo] 使用 USTC sparse crates 镜像"; \
    else \
        echo "[Cargo][WARN] USTC crates 镜像不可达，使用默认 crates.io"; \
    fi

ARG USER_ID=1000
ARG GROUP_ID=1000
RUN groupadd -g ${GROUP_ID} builder \
    && useradd -u ${USER_ID} -g builder -m -s /bin/bash builder \
    && echo "builder ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

WORKDIR /workspace
USER builder

CMD ["/bin/bash"]
