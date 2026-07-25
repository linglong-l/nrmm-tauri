#Requires -Version 5.1
<#
.SYNOPSIS
    配置 WSL2 环境以支持 nrmm-rust 的 AppImage 构建（解决 /dev/fuse 缺失问题）。

.DESCRIPTION
    参考官方文档 https://learn.microsoft.com/zh-cn/windows/wsl/wsl-config
    本脚本自动生成 %USERPROFILE%\.wslconfig，启用 FUSE、systemd、GUI 支持等关键配置，
    解决 Fedora/Ubuntu WSL2 中 /dev/fuse 缺失、linuxdeploy 无法运行导致 AppImage 构建失败的问题。

    运行后需要执行 `wsl --shutdown` 重启 WSL2 以使配置生效。

.PARAMETER NoBackup
    不备份已存在的 .wslconfig（默认会备份为 .wslconfig.nrmm-backup-时间戳）

.PARAMETER KeepExisting
    保留现有 .wslconfig 中的非冲突项（默认行为，推荐）

.PARAMETER ApplyNow
    写完后直接执行 wsl --shutdown 使配置立即生效（需要确认）

.EXAMPLE
    # 以管理员身份运行 PowerShell，然后执行：
    & .\scripts\setup-wsl2.ps1
    & .\scripts\setup-wsl2.ps1 -ApplyNow
#>

param(
    [switch]$NoBackup = $false,
    [switch]$KeepExisting = $true,
    [switch]$ApplyNow = $false
)

$ErrorActionPreference = "Stop"

function Write-Step([string]$msg) { Write-Host "`n===== $msg =====" -ForegroundColor Cyan }
function Write-Ok([string]$msg)   { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Warn([string]$msg) { Write-Host "  [WARN] $msg" -ForegroundColor Yellow }
function Write-Fail([string]$msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red }
function Write-Info([string]$msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }

Write-Step "nrmm-rust WSL2 环境配置脚本"
Write-Info "参考文档: https://learn.microsoft.com/zh-cn/windows/wsl/wsl-config"

# ============================================================
# 1. 检测 Windows 版本 & 管理员权限
# ============================================================
Write-Step "1/4 检测 Windows 与 WSL 环境"

$WinVer = [Environment]::OSVersion.Version
Write-Info "Windows 版本: $($WinVer.Major).$($WinVer.Minor).$($WinVer.Build)"

if ($WinVer.Build -lt 22000) {
    Write-Warn "检测到 Windows 10 (Build $($WinVer.Build))，部分 WSL2 高级功能（如 systemd）可能不完整"
    Write-Warn "建议升级到 Windows 11 (Build 22000+) 以获得最佳体验"
}

$IsAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $IsAdmin) {
    Write-Warn "当前未以管理员身份运行，可能无法写入 system32 配置或执行 wsl --shutdown"
    $resp = Read-Host "是否继续？(y/N)"
    if ($resp -notmatch '^[Yy]$') { Write-Host "已取消"; exit 1 }
} else {
    Write-Ok "当前以管理员身份运行"
}

# 检测 wsl.exe 是否存在
try {
    $WslVer = & wsl --version 2>&1
    Write-Ok "WSL 已安装: $($WslVer | Select-Object -First 1)"
} catch {
    Write-Fail "未检测到 wsl.exe，请先在 \"启用或关闭 Windows 功能\" 中启用 \"适用于 Linux 的 Windows 子系统\" 和 \"虚拟机平台\"，然后重启电脑"
    exit 1
}

# 检测默认 WSL 版本
try {
    $DefaultVer = & wsl --status 2>&1 | Select-String "默认版本"
    if ($DefaultVer) { Write-Info "WSL $DefaultVer" }
} catch {
    Write-Warn "无法获取 WSL 状态: $_"
}

# ============================================================
# 2. 读取现有 .wslconfig
# ============================================================
Write-Step "2/4 处理现有 .wslconfig 配置"

$WslConfigPath = Join-Path $env:USERPROFILE ".wslconfig"
$ExistingConfig = ""
$ExistingSections = @{}

if (Test-Path $WslConfigPath) {
    if (-not $NoBackup) {
        $BackupPath = Join-Path $env:USERPROFILE (".wslconfig.nrmm-backup-" + (Get-Date -Format "yyyyMMddHHmmss"))
        Copy-Item $WslConfigPath -Destination $BackupPath -Force
        Write-Ok "已备份现有配置: $BackupPath"
    } else {
        Write-Warn "已跳过备份（-NoBackup）"
    }

    # 简单解析现有 .wslconfig 为 hashtable（仅处理 [wsl2] section 的 key=value）
    try {
        $ExistingConfig = Get-Content $WslConfigPath -Raw -ErrorAction Stop
        $InWsl2 = $false
        foreach ($line in $ExistingConfig -split "`r?`n") {
            $trim = $line.Trim()
            if ($trim -match '^\[(.*)\]$') {
                $InWsl2 = ($Matches[1] -eq "wsl2")
                continue
            }
            if ($InWsl2 -and $trim -match '^([^=;#]+)=(.*)$') {
                $ExistingSections[$Matches[1].Trim()] = $Matches[2].Trim()
            }
        }
        if ($ExistingSections.Count -gt 0) {
            Write-Info "读取到现有 [wsl2] 配置项 $($ExistingSections.Count) 个"
        }
    } catch {
        Write-Warn "解析现有 .wslconfig 失败: $_，将覆盖写入"
    }
} else {
    Write-Info "未检测到现有 .wslconfig，将创建新文件"
}

# ============================================================
# 3. 生成目标配置（合并现有非冲突项）
# ============================================================
Write-Step "3/4 生成目标 .wslconfig"

$NrmmRequired = [ordered]@{
    # 启用 /dev/fuse 支持（AppImage 构建核心）
    "kernelCommandLine" = "fuse"

    # 启用 systemd（Fedora 44 WSL2 推荐开启，解决部分 systemd 依赖的软件包）
    "systemd" = "true"

    # GUI 支持 (WSLg)，透明窗口渲染依赖
    "guiApplications" = "true"

    # 内存/CPU 分配（根据机器自动调整，这里写保守默认：一半内存+所有CPU）
    # "memory" = "8GB"
    # "processors" = "4"

    # 启用 localhost 转发（Vite dev server 可从 Windows 浏览器访问）
    "localhostForwarding" = "true"

    # 嵌套虚拟化（Docker in WSL2 等场景需要，不影响 Tauri 构建）
    "nestedVirtualization" = "true"

    # 页面报告：减少 WSL2 在空闲时的内存占用
    "pageReporting" = "true"
}

$FinalCfg = [ordered]@{}
if ($KeepExisting) {
    foreach ($k in $ExistingSections.Keys) { $FinalCfg[$k] = $ExistingSections[$k] }
}
foreach ($k in $NrmmRequired.Keys) {
    if ($FinalCfg.Contains($k) -and $FinalCfg[$k] -ne $NrmmRequired[$k]) {
        Write-Warn "[wsl2] $k 现有值: $($FinalCfg[$k]) -> 覆盖为: $($NrmmRequired[$k])"
    }
    $FinalCfg[$k] = $NrmmRequired[$k]
}

$Output = @(
    "# Auto-generated by nrmm-rust scripts/setup-wsl2.ps1",
    "# 参考: https://learn.microsoft.com/zh-cn/windows/wsl/wsl-config",
    "# 生成时间: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')",
    "",
    "[wsl2]"
)
foreach ($k in $FinalCfg.Keys) {
    $Output += "  $k=$($FinalCfg[$k])"
}
$Output += ""
$Output += "[boot]"
$Output += "  systemd=true"
$Output += ""

$OutputContent = ($Output -join "`r`n")
Write-Info "目标配置内容:"
Write-Host ($OutputContent -split "`r?`n" | ForEach-Object { "    $_" }) -ForegroundColor DarkGray

try {
    Set-Content -Path $WslConfigPath -Value $OutputContent -Encoding UTF8 -Force
    Write-Ok "已写入配置文件: $WslConfigPath"
} catch {
    Write-Fail "写入 .wslconfig 失败: $_"
    exit 1
}

# ============================================================
# 4. 提示/执行 wsl --shutdown
# ============================================================
Write-Step "4/4 重启 WSL2 生效"

Write-Host ""
Write-Host "  ⚠️  必须重启 WSL2 才能让新配置生效！" -ForegroundColor Yellow
Write-Host "     重启命令: wsl --shutdown"
Write-Host ""

if ($ApplyNow) {
    $resp = Read-Host "确认立即执行 wsl --shutdown 吗？(y/N)"
    if ($resp -match '^[Yy]$') {
        Write-Info "执行 wsl --shutdown ..."
        try {
            & wsl --shutdown
            Write-Ok "WSL2 已关闭，重新进入 WSL2 后 /dev/fuse 应已可用"
            Write-Info "下一步: 在 WSL2 终端中执行 -> bash scripts/fix-appimage-env.sh --apply"
        } catch {
            Write-Fail "wsl --shutdown 执行失败: $_，请手动执行"
        }
    } else {
        Write-Warn "未执行重启，请稍后手动运行: wsl --shutdown"
    }
} else {
    Write-Info "请手动执行以下命令使配置生效："
    Write-Host ""
    Write-Host "    wsl --shutdown" -ForegroundColor Cyan
    Write-Host "    # 重新进入 WSL2，然后执行："
    Write-Host "    bash scripts/fix-appimage-env.sh --apply" -ForegroundColor Cyan
    Write-Host ""
}

Write-Step "完成"
exit 0
