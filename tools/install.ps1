# 一键安装：把 exe 放到用户目录、创建桌面快捷方式、可选写入开机自启
#
#   powershell -ExecutionPolicy Bypass -File tools\install.ps1
#   powershell -ExecutionPolicy Bypass -File tools\install.ps1 -AutoStart
#
param(
    [switch]$AutoStart
)

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repo 'target\release\sk-vitals.exe'
if (-not (Test-Path $exe)) {
    Write-Host "找不到 $exe，先执行 cargo build --release" -ForegroundColor Red
    exit 1
}

# 装到用户目录，避免放在仓库里被 cargo clean 掉
$installDir = Join-Path $env:LOCALAPPDATA 'SK-vitals'
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$installExe = Join-Path $installDir 'sk-vitals.exe'
Copy-Item $exe $installExe -Force
Write-Host "已安装到 $installExe" -ForegroundColor Green

# 桌面快捷方式
$desktop = [Environment]::GetFolderPath('Desktop')
$lnk = Join-Path $desktop 'SK-vitals.lnk'
$shell = New-Object -ComObject WScript.Shell
$sc = $shell.CreateShortcut($lnk)
$sc.TargetPath = $installExe
$sc.WorkingDirectory = $installDir
$sc.IconLocation = "$installExe,0"
$sc.Description = 'SK-vitals 任务栏资源监控'
$sc.Save()
Write-Host "已创建桌面快捷方式 $lnk" -ForegroundColor Green

# 开机自启（写 HKCU\...\Run，不需要管理员权限）
if ($AutoStart) {
    $runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    New-ItemProperty -Path $runKey -Name 'SK-vitals' -Value "`"$installExe`"" -PropertyType String -Force | Out-Null
    Write-Host "已加入开机自启" -ForegroundColor Green
}

Write-Host ""
Write-Host "启动：$installExe"
Write-Host "提示：移动 exe 或重装后，右键托盘图标可重新打开托盘菜单调整开关。"
