FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
ENV LANG=C.UTF-8
ENV LC_ALL=C.UTF-8

# ===== 配置阿里云 Ubuntu APT 镜像源 =====
RUN sed -i 's/archive.ubuntu.com/mirrors.aliyun.com/g' /etc/apt/sources.list \
    && sed -i 's/security.ubuntu.com/mirrors.aliyun.com/g' /etc/apt/sources.list \
    && apt-get update

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    pkg-config \
    curl \
    wget \
    file \
    git \
    ca-certificates \
    libwebkit2gtk-4.1-dev \
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
    && rm -rf /var/lib/apt/lists/*

# ===== 配置 Node.js 镜像源 (淘宝) =====
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/* \
    && npm config set registry https://registry.npmmirror.com \
    && npm install -g @tauri-apps/cli@2

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
