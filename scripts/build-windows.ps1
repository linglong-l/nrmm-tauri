#Requires -Version 5.1
<#
.SYNOPSIS
    NRMM Windows 手动构建脚本（与 build-linux-zig.sh 对等：手动编译 + 手动上传发布）。

.DESCRIPTION
    执行流程：
      1. 前置工具检查（node/cargo/npm）
      2. 版本一致性校验（package.json / src-tauri/Cargo.toml / tauri.conf.json /
         tauri.windows.conf.json / tauri.linux.conf.json，项目硬约束：五处版本号必须一致）
      3. 7z Windows sidecar 检查/下载（binaries 目录被 gitignore，新检出环境自补：
         7zr.exe 引导 + 7z2602-extra.7z 解压，布局与 tauri.windows.conf.json resources 映射一致）
      4. npm 依赖检查（缺失时 npm ci）
      5. Tauri 构建（nsis + msi，显式 --target 保证产物路径确定性）
      6. portable zip（复用 build-portable.mjs，含主程序 + DLL + resources）
      7. 收集产物到 dist-win/：安装包 / msi / portable zip / latest.json（updater manifest）

    构建完成后请手动上传 dist-win/ 内容到 Gitee Release（tag: v*.*.*）。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/build-windows.ps1
    # 或 npm run build:windows
#>

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectRoot

$TargetTriple = "x86_64-pc-windows-msvc"
$DistDir = "dist-win"
$ArchiveMirror = "https://www.7-zip.org/a"

# ---------- 辅助函数 ----------
function Write-Step([string]$msg) { Write-Host "`n===== $msg =====" -ForegroundColor Cyan }
function Write-Ok([string]$msg)   { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Warn2([string]$msg){ Write-Host "  [WARN] $msg" -ForegroundColor Yellow }
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
    "package.json"             = $pkgVersion
    "src-tauri/Cargo.toml"     = $cargoVersion
    "src-tauri/tauri.conf.json"     = $confBase.version
    "src-tauri/tauri.windows.conf.json" = $confWin.version
    "src-tauri/tauri.linux.conf.json"  = $confLinux.version
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
$sevenZipRoot = "src-tauri/binaries/7z/7z2602-Windows"
$required = @(
    "x64/7za.exe", "x64/7za.dll", "x64/7zxa.dll",
    "x32/7za.exe", "x32/7za.dll", "x32/7zxa.dll"
)
$missing = $required | Where-Object { -not (Test-Path (Join-Path $sevenZipRoot $_)) }
if (-not $missing) {
    Write-Ok "7z sidecar 完整（x64/x32）"
} else {
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

# ---------- 4. npm 依赖检查 ----------
Write-Step "[4/7] npm 依赖检查"
if (-not (Test-Path "node_modules")) {
    Write-Host "  安装 npm 依赖（npm ci）..."
    npm ci
    if ($LASTEXITCODE -ne 0) { Write-Fail "npm ci 失败" }
} else {
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

Copy-FirstFound "nrmm-rust-x86_64-setup.exe" @((Join-Path $bundleDir "nsis/*-setup.exe"))
Copy-FirstFound "nrmm-rust-x86_64.msi"       @((Join-Path $bundleDir "msi/*.msi"))
# updater manifest（Tauri v2 生成位置随版本可能为 bundle 根或 nsis/msi 子目录）
Copy-FirstFound "latest-windows.json"        @((Join-Path $bundleDir "latest.json"), (Join-Path $bundleDir "*/latest.json"))
# portable zip 已由 build-portable.mjs 直接输出到 dist-win/

Write-Host ""
Write-Host "===== 构建完成 =====" -ForegroundColor Cyan
Get-ChildItem $DistDir | ForEach-Object {
    Write-Host ("  {0}  {1:N2} MB" -f $_.Name, ($_.Length / 1MB))
}
Write-Host ""
Write-Host "请手动上传以上产物到 Gitee Release（tag: v$pkgVersion），latest-windows.json 用于 updater manifest。" -ForegroundColor Yellow
