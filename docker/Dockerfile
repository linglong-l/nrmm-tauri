FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
ENV LANG=C.UTF-8
ENV LC_ALL=C.UTF-8

# ===== 配置阿里云 Ubuntu APT 镜像源，并在单层完成 update + install =====
# Tauri 在 Ubuntu 22.04 上优先使用 libwebkit2gtk-4.1-dev；如果镜像源未同步（阿里云部分节点），
# 则回退到 libwebkit2gtk-4.0-dev（同样支持 Tauri v2）。
RUN sed -i 's/archive.ubuntu.com/mirrors.aliyun.com/g' /etc/apt/sources.list \
    && sed -i 's/security.ubuntu.com/mirrors.aliyun.com/g' /etc/apt/sources.list \
    && apt-get update \
    && (apt-get install -y --no-install-recommends libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev \
        || apt-get install -y --no-install-recommends libwebkit2gtk-4.0-dev libjavascriptcoregtk-4.0-dev) \
    && apt-get install -y --no-install-recommends \
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
    && apt-get install -f -y \
    && rm -rf /var/lib/apt/lists/*

# ===== 安装 Node.js 20 + 配置淘宝 npm 镜像 =====
# 优先通过 nodesource 官方脚本安装；如果 deb.nodesource.com 在国内访问失败，
# 回退到直接下载 nodejs tarball 到 /usr/local（与 npmmirror 镜像同步）。
RUN set -eux; \
    if curl -fsSL --max-time 30 https://deb.nodesource.com/setup_20.x | bash -; then \
        apt-get update && apt-get install -y --no-install-recommends nodejs; \
    else \
        echo "[WARN] nodesource 脚本失败，改用淘宝镜像下载 Node.js"; \
        NODE_VERSION=v20.19.0; \
        curl -fsSL --max-time 120 -o /tmp/node.tar.xz \
            "https://registry.npmmirror.com/-/binary/node/${NODE_VERSION}/node-${NODE_VERSION}-linux-x64.tar.xz"; \
        tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1; \
        rm -f /tmp/node.tar.xz; \
    fi; \
    rm -rf /var/lib/apt/lists/*; \
    npm config set registry https://registry.npmmirror.com; \
    npm install -g @tauri-apps/cli@2

ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH

# ===== 配置阿里云 Rust 镜像源 =====
ENV RUSTUP_DIST_SERVER=https://mirrors.aliyun.com/rustup
ENV RUSTUP_UPDATE_ROOT=https://mirrors.aliyun.com/rustup/rustup

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable \
    && rustup target add x86_64-unknown-linux-gnu \
    && chmod -R a+w $RUSTUP_HOME $CARGO_HOME

# ===== 配置 Cargo crates 镜像源 (USTC) =====
RUN mkdir -p $CARGO_HOME \
    && echo '[source.crates-io]' > $CARGO_HOME/config.toml \
    && echo 'replace-with = "ustc"' >> $CARGO_HOME/config.toml \
    && echo '' >> $CARGO_HOME/config.toml \
    && echo '[source.ustc]' >> $CARGO_HOME/config.toml \
    && echo 'registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"' >> $CARGO_HOME/config.toml

ARG USER_ID=1000
ARG GROUP_ID=1000
RUN groupadd -g ${GROUP_ID} builder \
    && useradd -u ${USER_ID} -g builder -m -s /bin/bash builder \
    && echo "builder ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

WORKDIR /workspace
USER builder

CMD ["/bin/bash"]
