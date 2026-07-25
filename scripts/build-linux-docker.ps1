#Requires -Version 5.1
<#
.SYNOPSIS
    在 Docker Desktop 容器中构建 nrmm-rust 的 Linux 安装包 (AppImage/deb/rpm)。

.DESCRIPTION
    使用 Ubuntu 22.04 Docker 镜像构建 Tauri Linux 包，确保 AppImage 跨发行版兼容性。
    需安装 Docker Desktop 并已启动。

.PARAMETER AppImage
    仅构建 AppImage

.PARAMETER Deb
    仅构建 deb 包

.PARAMETER Rpm
    仅构建 rpm 包

.PARAMETER NoRebuild
    不重新构建 Docker 镜像，使用已有的

.PARAMETER Shell
    进入容器交互 Shell 进行调试

.EXAMPLE
    .\scripts\build-linux-docker.ps1
    .\scripts\build-linux-docker.ps1 -AppImage
    .\scripts\build-linux-docker.ps1 -Shell
#>

param(
    [switch]$AppImage = $false,
    [switch]$Deb = $false,
    [switch]$Rpm = $false,
    [switch]$NoRebuild = $false,
    [switch]$Shell = $false
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ImageName = "nrmm-tauri-builder:latest"
$Dockerfile = Join-Path $ProjectRoot "docker\Dockerfile"
$OutputDir = Join-Path $ProjectRoot "dist-linux"
$Target = "x86_64-unknown-linux-gnu"

function Write-Step([string]$msg) { Write-Host "`n===== $msg =====" -ForegroundColor Cyan }
function Write-Ok([string]$msg)   { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Fail([string]$msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red }

Write-Step "检查 Docker"
try {
    docker info | Out-Null
    Write-Ok "Docker 可用"
} catch {
    Write-Fail "Docker 不可用，请先启动 Docker Desktop"
    exit 1
}

$BundleArgs = "--bundles deb,rpm,appimage"
if ($AppImage) { $BundleArgs = "--bundles appimage" }
elseif ($Deb) { $BundleArgs = "--bundles deb" }
elseif ($Rpm) { $BundleArgs = "--bundles rpm" }

if (-not $NoRebuild) {
    Write-Step "构建 Docker 镜像 ($ImageName)"
    docker build -t $ImageName -f $Dockerfile "$ProjectRoot\docker"
    if ($LASTEXITCODE -ne 0) { Write-Fail "Docker 镜像构建失败"; exit 1 }
    Write-Ok "Docker 镜像构建完成"
}

if ($Shell) {
    Write-Step "进入容器 Shell (调试模式)"
    docker run --rm -it `
        -v "${ProjectRoot}:/workspace" `
        -v "nrmm-cargo-registry:/usr/local/cargo/registry" `
        -v "nrmm-cargo-git:/usr/local/cargo/git" `
        -v "nrmm-rustup:/usr/local/rustup" `
        -v "nrmm-npm-cache:/home/builder/.npm" `
        -u builder `
        -w /workspace `
        $ImageName /bin/bash
    exit 0
}

Write-Step "准备输出目录"
if (-not (Test-Path $OutputDir)) { New-Item -ItemType Directory -Path $OutputDir | Out-Null }

Write-Step "在容器中构建 Tauri Linux 包"
$BuildCmd = @"
set -e
echo '--- 安装 npm 依赖 ---'
npm ci
echo '--- 前端构建 ---'
npm run build
echo '--- Tauri Linux 构建 ---'
npx tauri build --target $Target $BundleArgs
echo '--- 构建完成 ---'
"@

docker run --rm `
    -v "${ProjectRoot}:/workspace" `
    -v "nrmm-cargo-registry:/usr/local/cargo/registry" `
    -v "nrmm-cargo-git:/usr/local/cargo/git" `
    -v "nrmm-rustup:/usr/local/rustup" `
    -v "nrmm-npm-cache:/home/builder/.npm" `
    -u builder `
    -w /workspace `
    -e CARGO_PROFILE_RELEASE_LTO=true `
    -e CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 `
    -e CARGO_PROFILE_RELEASE_STRIP=true `
    $ImageName /bin/bash -c $BuildCmd

if ($LASTEXITCODE -ne 0) { Write-Fail "Tauri 构建失败"; exit 1 }
Write-Ok "构建完成"

Write-Step "收集构建产物"
$BundleDir = Join-Path $ProjectRoot "src-tauri\target\$Target\release\bundle"

function Copy-Pattern($Dir, $Pattern, $DestName) {
    $file = Get-ChildItem -Path $Dir -Filter $Pattern -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($file) {
        Copy-Item $file.FullName -Destination (Join-Path $OutputDir $DestName)
        Write-Ok "$($file.Name) -> $DestName"
    } else {
        Write-Host "  [WARN] 未找到 $Pattern in $Dir" -ForegroundColor Yellow
    }
}

Copy-Pattern (Join-Path $BundleDir "appimage") "*.AppImage" "nrmm-rust-x86_64.AppImage"
Copy-Pattern (Join-Path $BundleDir "deb") "*.deb" "nrmm-rust-x86_64.deb"
Copy-Pattern (Join-Path $BundleDir "rpm") "*.rpm" "nrmm-rust-x86_64.rpm"

$latest = Get-ChildItem -Path $BundleDir -Filter "latest.json" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
if ($latest) {
    Copy-Item $latest.FullName -Destination (Join-Path $OutputDir "latest-linux.json")
    Write-Ok "latest-linux.json"
}

Write-Host "`n--- dist-linux/ 内容 ---" -ForegroundColor DarkGray
Get-ChildItem $OutputDir | Format-Table Name, @{N="Size(MB)";E={[math]::Round($_.Length/1MB,2)}} -AutoSize

Write-Host "`n===== 构建完成 =====" -ForegroundColor Green
Write-Host "产物位于: $OutputDir" -ForegroundColor Green
