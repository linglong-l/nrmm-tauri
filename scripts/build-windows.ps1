#Requires -Version 7

# param 必须是脚本的首个可执行语句（#Requires 与注释可前置）。此前置于编码设置之后，
# PowerShell 报「术语 'param' 不会被识别为 cmdlet...」→ -TargetTriple 无法传入。
param([string]$TargetTriple = "x86_64-pc-windows-msvc")

# 强制控制台输入/输出编码为 UTF-8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
[Console]::InputEncoding  = [System.Text.Encoding]::UTF8
$OutputEncoding            = [System.Text.Encoding]::UTF8

# (可选) 如果是在 Windows 上，建议同时设置代码页为 65001
# 注意：chcp 仅对当前控制台窗口有效，不影响管道传输
if ($IsWindows) {
    chcp 65001 | Out-Null
}
<#
.SYNOPSIS
    NRMM Windows 手动构建脚本（与 build-linux-gnu.sh 对等：手动编译 + 手动上传发布）。

.DESCRIPTION
    执行流程：
      1. 前置工具检查（node/cargo/npm）
      2. 版本一致性校验（package.json / src-tauri/Cargo.toml / tauri.conf.json /
         tauri.windows.conf.json / tauri.linux.conf.json，项目硬约束：五处版本号必须一致）
      3. 7z Windows sidecar 检查/下载（binaries 目录被 gitignore，新检出环境自补：
         7zr.exe 引导 + 7z2602-extra.7z 解压，布局与 tauri.windows.conf.json resources 映射一致）
      4. npm 依赖检查（缺失时 npm ci）
      5. Tauri 构建（nsis，显式 --target 保证产物路径确定性；已移除 msi）
      6. portable zip（复用 build-portable.mjs，含主程序 + DLL + 内置 7z，命名为规范名）
      7. 收集产物到 dist-win/：安装包 / portable zip / latest.json（updater manifest）

    构建完成后请手动上传 dist-win/ 内容到 Gitee Release（tag: v*.*.*）。
    产物文件名按 nrmm-tauri_<版本>_x64-Win.<ext> 规范；Windows 仅支持 x86_64。

.EXAMPLE
    pwsh -ExecutionPolicy Bypass -File scripts/build-windows.ps1
    # 或 npm run build:windows（必须 pwsh 7+：脚本含中文，Windows PowerShell 5.1 按 ANSI
    # 解码无 BOM 的 UTF-8 脚本会乱码致解析失败，且 chcp 65001 不改变脚本读取编码）
#>

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectRoot

# $TargetTriple 由 param 块/用户传入；默认 x86_64-pc-windows-msvc（见脚本第一行 param）
$DistDir = "dist-win"
$ArchiveMirror = "https://www.7-zip.org/a"

# 由目标 triple 派生的架构标识：i686 -> x32/i686，其余（含 x86_64/aarch64）-> x64/x86_64
$ArchSuffix = if ($TargetTriple -like "i686*") { "i686" } else { "x86_64" }

# ---------- 0. 更新签名密钥环境（确保 updater manifest 自动生成） ----------
# Tauri 仅在构建时检测到 TAURI_SIGNING_PRIVATE_KEY 才会生成 latest.json 与签名更新包。
# 若环境未设置，则从仓库内置 Key/Key.Private 载入；同时清除可能冲突的
# TAURI_SIGNING_PRIVATE_KEY_PATH（二者在 `tauri signer sign` 子命令下互斥）。
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
    $keyPath = Join-Path $ProjectRoot "Key/Key.Private"
    if (Test-Path $keyPath) {
        $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $keyPath -Raw).Trim()
        Write-Ok "已从 Key/Key.Private 载入签名私钥"
    }
    else {
        Write-Warn2 "未设置 TAURI_SIGNING_PRIVATE_KEY 且 Key/Key.Private 缺失，updater manifest 将不会生成"
    }
}
if ($env:TAURI_SIGNING_PRIVATE_KEY -and $env:TAURI_SIGNING_PRIVATE_KEY_PATH) {
    Remove-Item env:TAURI_SIGNING_PRIVATE_KEY_PATH -ErrorAction SilentlyContinue
}
if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
    Write-Warn2 "TAURI_SIGNING_PRIVATE_KEY_PASSWORD 未设置（内置私钥已加密），签名可能失败"
}

# ---------- 辅助函数 ----------
function Write-Step([string]$msg) { Write-Host "`n===== $msg =====" -ForegroundColor Cyan }
function Write-Ok([string]$msg) { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Warn2([string]$msg) { Write-Host "  [WARN] $msg" -ForegroundColor Yellow }
function Write-Fail([string]$msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; exit 1 }

# ---------- 1. 前置工具检查 ----------
Write-Step "[1/7] 前置工具检查"
foreach ($tool in @("node", "npm", "cargo", "rustup")) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Fail "$tool 不在 PATH"
    }
}
node --version
cargo --version

# Rust target 检查（宿主通常已装，防御性补装）
$installed = rustup target list --installed 2>$null
if ($installed -notmatch [regex]::Escape($TargetTriple)) {
    Write-Host "  安装 Rust target: $TargetTriple"
    rustup target add $TargetTriple
    if ($LASTEXITCODE -ne 0) { Write-Fail "rustup target add 失败" }
}
Write-Ok "工具链就绪"

# ---------- 2. 版本一致性校验（项目硬约束） ----------
Write-Step "[2/7] 版本一致性校验"
$pkgVersion = (Get-Content "package.json" -Raw | ConvertFrom-Json).version
$confWin = Get-Content "src-tauri/tauri.windows.conf.json" -Raw | ConvertFrom-Json
$confBase = Get-Content "src-tauri/tauri.conf.json" -Raw | ConvertFrom-Json
$confLinux = Get-Content "src-tauri/tauri.linux.conf.json" -Raw | ConvertFrom-Json
$cargoVersion = (Select-String -Path "src-tauri/Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value

$versions = [ordered]@{
    "package.json"                      = $pkgVersion
    "src-tauri/Cargo.toml"              = $cargoVersion
    "src-tauri/tauri.conf.json"         = $confBase.version
    "src-tauri/tauri.windows.conf.json" = $confWin.version
    "src-tauri/tauri.linux.conf.json"   = $confLinux.version
}
$mismatch = $versions.GetEnumerator() | Where-Object { $_.Value -ne $pkgVersion }
if ($mismatch) {
    foreach ($kv in $versions.GetEnumerator()) {
        $mark = if ($kv.Value -eq $pkgVersion) { "  " } else { "!=" }
        Write-Host ("  {0} {1} = {2}" -f $mark, $kv.Key, $kv.Value)
    }
    Write-Fail "版本号不一致（发布硬约束：五处必须一致，基准 package.json=$pkgVersion）"
}
Write-Ok "五处版本号一致: $pkgVersion"

# ---------- 3. 7z Windows sidecar 检查/下载 ----------
Write-Step "[3/7] 7z Windows sidecar 检查"
$sevenZipRoot = "src-tauri/binaries/7z/Windows-x64"
$required = @(
    "7za.exe", "7za.dll", "7zxa.dll"
)
$missing = $required | Where-Object { -not (Test-Path (Join-Path $sevenZipRoot $_)) }
if (-not $missing) {
    Write-Ok "7z sidecar 完整（无需下载补齐）"
}
else {
    Write-Warn2 "缺失: $($missing -join ', ')，开始下载补齐（7zr.exe 引导 + 7z2602-extra.7z）"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $tmp = Join-Path $env:TEMP "nrmm-7z-bootstrap"
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null

    $sevenZr = Join-Path $tmp "7zr.exe"
    Invoke-WebRequest -Uri "$ArchiveMirror/7zr.exe" -OutFile $sevenZr -UseBasicParsing
    $extraArchive = Join-Path $tmp "7z2602-extra.7z"
    Invoke-WebRequest -Uri "$ArchiveMirror/7z2602-extra.7z" -OutFile $extraArchive -UseBasicParsing

    New-Item -ItemType Directory -Force -Path $sevenZipRoot | Out-Null
    & $sevenZr x -y ("-o$sevenZipRoot") $extraArchive | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "7z extra 包解压失败" }
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue

    # extra 包根目录即含 x64/ x32/，与 resources 映射布局一致；复验完整性
    $stillMissing = $required | Where-Object { -not (Test-Path (Join-Path $sevenZipRoot $_)) }
    if ($stillMissing) { Write-Fail "下载补齐后仍缺失: $($stillMissing -join ', ')" }
    Write-Ok "7z sidecar 已补齐"
}

# 注：tauri.windows.conf.json 的 resources 已直接指向 Windows-x64/（无需再 staging 到 current/），
# 因此 7z 二进制就地位于 $sevenZipRoot，Tauri 构建时直接打包。

# ---------- 4. npm 依赖检查 ----------
Write-Step "[4/7] npm 依赖检查"
if (-not (Test-Path "node_modules")) {
    Write-Host "  安装 npm 依赖（npm ci）..."
    npm ci
    if ($LASTEXITCODE -ne 0) { Write-Fail "npm ci 失败" }
}
else {
    Write-Ok "node_modules 存在"
}

# ---------- 5. Tauri 构建（nsis + msi；显式 --target 保证 bundle 路径确定性） ----------
# 注意：必须用 npx tauri 而非 `npm run tauri build -- --target X`——npm 会把脚本名后的
# `--target` 当成本身配置吞掉（Unknown cli config --target），只把值当位置参数传给 tauri，
# 导致 cargo 收到裸 triple 报 "unexpected argument"。npx 直传无此问题（2026-08-18 实测）。
Write-Step "[5/7] Tauri 构建（targets: nsis + msi，tauri.windows.conf.json）"
npx tauri build --target $TargetTriple
if ($LASTEXITCODE -ne 0) { Write-Fail "tauri build 失败（exit=$LASTEXITCODE）" }

# ---------- 6. portable zip ----------
Write-Step "[6/7] portable 打包"
node scripts/build-portable.mjs --target $TargetTriple -o $DistDir
if ($LASTEXITCODE -ne 0) { Write-Fail "portable 打包失败" }

# ---------- 7. 收集产物 ----------
Write-Step "[7/7] 收集产物到 $DistDir/"
# workspace 根 target 优先，回退 src-tauri/target（与 build-portable.mjs 探测逻辑一致）
$releaseDir = @("target/$TargetTriple/release", "src-tauri/target/$TargetTriple/release") |
Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $releaseDir) { Write-Fail "找不到 release 目录（target/ 与 src-tauri/target/ 均无）" }
$bundleDir = Join-Path $releaseDir "bundle"

New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
function Copy-FirstFound([string]$destName, [string[]]$globs) {
    foreach ($glob in $globs) {
        $found = Get-ChildItem $glob -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found) {
            Copy-Item $found.FullName (Join-Path $DistDir $destName) -Force
            Write-Ok $destName
            return
        }
    }
    Write-Warn2 "未找到: $destName"
}

# 安装包改名到规范名：nrmm-tauri_<版本>_x64-Win.exe（msi 已移除）
Copy-FirstFound "nrmm-tauri_${pkgVersion}_x64-Win.exe" @((Join-Path $bundleDir "nsis/*-setup.exe"))
# portable zip 已由 build-portable.mjs 直接输出到 dist-win/（nrmm-tauri_<版本>_x64-Win.zip）

# ---------- 生成 updater manifest（latest.json）----------
# 说明：Tauri 的 `tauri build` 不会写出 latest.json，也不会生成独立更新包；
# 更新直接复用安装包本体，由本脚本现场签名（*.sig）并生成 manifest。
Write-Step "[7b] 生成 updater manifest（latest.json）"
$env:ARCH_SUFFIX = $ArchSuffix  # gen-latest-json.mjs 据此派生安装包名与平台键
$genOut = node scripts/gen-latest-json.mjs 2>&1
$genOut | ForEach-Object { Write-Host "  $_" }
if (-not (Test-Path (Join-Path $DistDir "latest.json"))) { Write-Fail "latest.json 生成失败" }

Write-Host ""
Write-Host "===== 构建完成 =====" -ForegroundColor Cyan
Get-ChildItem $DistDir | ForEach-Object {
    Write-Host ("  {0}  {1:N2} MB" -f $_.Name, ($_.Length / 1MB))
}
Write-Host ""
Write-Host "请手动上传以上产物到 Gitee Release（tag: v$pkgVersion），latest-windows.json 用于 updater manifest。" -ForegroundColor Yellow
