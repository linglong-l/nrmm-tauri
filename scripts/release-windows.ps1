#Requires -Version 5.1
<#
.SYNOPSIS
    在本地 Windows 环境构建 nrmm-rust 并上传到 Gitee Release。

.DESCRIPTION
    执行流程：
      1. 检查必要工具（node/cargo/npm/tauri CLI）
      2. 校验版本一致性（npm run verify:release）
      3. 执行 Tauri Windows 构建（NSIS 安装包 + latest.json）
      4. 收集产物到 dist-win/ 目录
      5. 如果 Release 已存在（由 Gitee Go 云端 Linux 构建创建），补传 Windows 产物；
         否则调用 gitee-release.mjs 创建 Release 并上传。

.PARAMETER Tag
    版本标签，如 v0.3.0。若不指定，自动从 tauri.conf.json 的 version 字段生成（加 v 前缀）。

.PARAMETER SkipBuild
    跳过构建步骤，仅上传 dist-win/ 中已存在的产物。

.PARAMETER Force
    强制重新上传同名附件（先删除旧的）。

.PARAMETER UploadOnly
    仅上传模式：不尝试创建 Release（假设已由 CI 创建）。

.PARAMETER Token
    Gitee 私人令牌。若未提供则从 $env:GITEE_TOKEN 读取。

.EXAMPLE
    # 首次构建并发布（本地独立完成）
    .\scripts\release-windows.ps1 -Token "your_token_here"

.EXAMPLE
    # Gitee Go 已在云端创建 Release + Linux 产物，本地补传 Windows 产物
    .\scripts\release-windows.ps1 -UploadOnly -Tag v0.3.0

.EXAMPLE
    # 仅重新上传（产物已在 dist-win/）
    .\scripts\release-windows.ps1 -SkipBuild -UploadOnly -Tag v0.3.0 -Force
#>

param(
    [string]$Tag = "",
    [switch]$SkipBuild = $false,
    [switch]$Force = $false,
    [switch]$UploadOnly = $false,
    [string]$Token = ""
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectRoot

# ---------- 颜色辅助函数 ----------
function Write-Step([string]$msg) { Write-Host "`n===== $msg =====" -ForegroundColor Cyan }
function Write-Ok([string]$msg)   { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Warn2([string]$msg){ Write-Host "  [WARN] $msg" -ForegroundColor Yellow }
function Write-Fail([string]$msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red }

# ---------- 外部命令执行（避免 PS5 将 stderr 当致命错误） ----------
# 用法: $code = Invoke-Native { npm run tauri build -- --target x86_64-pc-windows-msvc }
# ScriptBlock 内直接写原生命令，参数传递不受 PowerShell 函数参数解析干扰。
function Invoke-Native {
    param(
        [Parameter(Mandatory=$true, Position=0)][scriptblock]$ScriptBlock
    )
    $oldEAP = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $ScriptBlock 2>&1 | Out-Host
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $oldEAP
    }
}

# ---------- 检查 Token ----------
if (-not $Token) { $Token = $env:GITEE_TOKEN }
if (-not $Token) {
    Write-Fail "未提供 Gitee Token。请通过 -Token 参数或 `$env:GITEE_TOKEN 环境变量设置。"
    Write-Host ""
    Write-Host "获取 Token：https://gitee.com/profile/personal_access_tokens" -ForegroundColor Yellow
    Write-Host "  需勾选 projects 权限。生成后请妥善保存（只显示一次）。"
    exit 1
}
$env:GITEE_TOKEN = $Token

# ---------- 检查必要命令 ----------
Write-Step "环境检查"
function Test-Command([string]$name) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if (-not $cmd) { Write-Fail "未找到 $name，请先安装"; exit 1 }
    Write-Ok "$name -> $($cmd.Source)"
}
Test-Command "node"
Test-Command "npm"
Test-Command "cargo"
Test-Command "rustc"

# ---------- 自动获取 Tag ----------
if (-not $Tag) {
    Write-Step "自动检测版本号"
    $conf = Get-Content "src-tauri/tauri.conf.json" -Raw | ConvertFrom-Json
    $ver = $conf.version
    if (-not $ver) { Write-Fail "无法从 tauri.conf.json 读取 version"; exit 1 }
    $Tag = "v$ver"
    Write-Ok "检测到版本 $ver -> Tag $Tag"
} else {
    Write-Ok "使用指定 Tag: $Tag"
}
$env:GITEE_TAG = $Tag

# ---------- 构建 ----------
$DistDir = Join-Path $ProjectRoot "dist-win"
if (-not $SkipBuild) {
    Write-Step "安装 npm 依赖"
    $exitCode = Invoke-Native { npm ci }
    if ($exitCode -ne 0) { Write-Fail "npm ci 失败 (exit code: $exitCode)"; exit 1 }
    Write-Ok "npm 依赖安装完成"

    Write-Step "构建前校验 (npm run verify:release)"
    $exitCode = Invoke-Native { npm run verify:release }
    if ($exitCode -ne 0) { Write-Fail "版本校验失败，请修复后重试"; exit 1 }
    Write-Ok "校验通过"

    Write-Step "Tauri Windows 构建 (x86_64-pc-windows-msvc)"
    $exitCode = Invoke-Native { npm run tauri build -- --target x86_64-pc-windows-msvc }
    if ($exitCode -ne 0) { Write-Fail "Tauri 构建失败 (exit code: $exitCode)"; exit 1 }
    Write-Ok "构建完成"

    Write-Step "收集 Windows 产物"
    if (Test-Path $DistDir) { Remove-Item $DistDir\* -Force -ErrorAction SilentlyContinue }
    else { New-Item -ItemType Directory -Path $DistDir | Out-Null }

    $BundleDir = "src-tauri/target/x86_64-pc-windows-msvc/release/bundle"
    $nsis = Get-ChildItem -Path "$BundleDir/nsis/*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($nsis) {
        Copy-Item $nsis.FullName -Destination "$DistDir/nrmm-rust-setup-x86_64.exe"
        Write-Ok "NSIS 安装包 -> nrmm-rust-setup-x86_64.exe"
    } else {
        Write-Warn2 "未找到 NSIS 安装包（$BundleDir/nsis/*.exe）"
    }

    $msiFiles = Get-ChildItem -Path "$BundleDir/msi/*.msi" -ErrorAction SilentlyContinue
    if ($msiFiles) {
        foreach ($msi in $msiFiles) {
            if ($msi.Name -match "(en-US|zh-CN)") {
                $lang = $Matches[1]
                $destName = "nrmm-rust-x86_64-$lang.msi"
            } else {
                $destName = "nrmm-rust-x86_64.msi"
            }
            Copy-Item $msi.FullName -Destination "$DistDir/$destName"
            Write-Ok "MSI 安装包 -> $destName"
        }
    } else {
        Write-Warn2 "未找到 MSI 安装包（$BundleDir/msi/*.msi），可能需要安装 WiX Toolset"
    }

    Write-Step "生成 Windows 便携版 (zip)"
    $exitCode = Invoke-Native { node scripts/build-portable.mjs --target x86_64-pc-windows-msvc -o $DistDir }
    if ($exitCode -ne 0) {
        Write-Warn2 "便携版 zip 生成失败（不影响安装包上传）"
    } else {
        Write-Ok "便携版 zip 已生成"
    }

    $latest = Get-ChildItem -Path $BundleDir -Filter "latest.json" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($latest) {
        Copy-Item $latest.FullName -Destination "$DistDir/latest-windows.json"
        Write-Ok "updater manifest -> latest-windows.json"
    } else {
        Write-Warn2 "未找到 latest.json（Tauri 签名未配置？）"
    }

    # 如果同时有 Linux latest.json（dist/ 目录），合并；否则只用 Windows 的
    $WinLatest = "$DistDir/latest-windows.json"
    $LinLatest = Join-Path $ProjectRoot "dist/latest-linux.json"
    $MergeInputs = @()
    if (Test-Path $WinLatest) { $MergeInputs += $WinLatest }
    if (Test-Path $LinLatest) { $MergeInputs += $LinLatest }
    if ($MergeInputs.Count -gt 0) {
        Write-Step "合并多平台 latest.json"
        $OutLatest = Join-Path $ProjectRoot "latest.json"
        $exitCode = Invoke-Native { node scripts/merge-updater-manifest.mjs @MergeInputs -o $OutLatest }
        if ($exitCode -eq 0 -and (Test-Path $OutLatest)) {
            Copy-Item $OutLatest -Destination "$DistDir/latest.json" -Force
            Write-Ok "已生成合并后的 latest.json"
            Get-Content $OutLatest
        }
    }

    Write-Host "`n--- dist-win/ 内容 ---" -ForegroundColor DarkGray
    Get-ChildItem $DistDir | Format-Table Name, @{N="Size(MB)";E={[math]::Round($_.Length/1MB,2)}} -AutoSize
} else {
    Write-Warn2 "跳过构建步骤（-SkipBuild）"
    if (-not (Test-Path $DistDir)) {
        Write-Fail "dist-win/ 不存在，无法上传"; exit 1
    }
}

# ---------- 上传到 Gitee Release ----------
Write-Step "上传 Windows 产物到 Gitee Release"
$UploadArgs = @("scripts/gitee-release.mjs", $DistDir, "--tag", $Tag)
if ($UploadOnly) { $UploadArgs += "--upload-only" }
if ($Force) { $UploadArgs += "--force" }

$exitCode = Invoke-Native { node @UploadArgs }
if ($exitCode -ne 0) {
    Write-Fail "上传失败，请检查错误信息"
    exit 1
}

Write-Host "`n============================================" -ForegroundColor Green
Write-Host " Windows 发布完成！" -ForegroundColor Green
Write-Host " Tag:     $Tag" -ForegroundColor Green
Write-Host " Release: https://gitee.com/Yezi26/nrmm-tauri/releases/$Tag" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
