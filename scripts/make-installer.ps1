<#
.SYNOPSIS
    构建 Windows 安装包（Inno Setup）与便携 ZIP。

.DESCRIPTION
    和 macOS 的 scripts/make-dmg.sh 对应，做三件事：
      1. 编译 release 版 mns.exe（CLI）与 mns-gui.exe（桌面端，--features gui）
      2. 把两个可执行文件、LICENSE 放进 target\dist\MNS-<version>-win-x64\
         并打一个便携 ZIP
      3. 用 Inno Setup 编译 MNS-<version>-win-x64-setup.exe 安装包

    按用户安装（%LOCALAPPDATA%\Programs\MNS），不弹 UAC；安装包附带开始菜单
    快捷方式与可选桌面快捷方式，不修改 PATH。

    CLI 必须和 GUI 放在同一目录：mns-gui 通过自身所在目录定位 mns.exe，
    再把它交给外接 Agent。

.PARAMETER Target
    Rust 目标三元组，默认取 rustc 的 host。只有 x64 能生成安装包，
    其它架构只生成便携 ZIP。

.PARAMETER InnoSetupPath
    ISCC.exe 的路径。默认依次查找 PATH、%ISCC%、常见安装目录和注册表。

.PARAMETER SkipBuild
    跳过 cargo 编译，直接用 target\release 里已有的产物（适合打包前刚构建过）。

.PARAMETER PortableOnly
    只生成便携 ZIP，不编译安装包。

.PARAMETER RequireInstaller
    找不到 Inno Setup 时报错退出，而不是只提示并留下 ZIP。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\make-installer.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\make-installer.ps1 -SkipBuild

.NOTES
    首次使用需安装 Inno Setup 6：
        winget install -e --id JRSoftware.InnoSetup
#>
[CmdletBinding()]
param(
    [string]$Target = '',
    [string]$InnoSetupPath = '',
    [switch]$SkipBuild,
    [switch]$PortableOnly,
    [switch]$RequireInstaller
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$distDir = Join-Path $root 'target\dist'

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Warn {
    param([string]$Message)
    Write-Host "warning: $Message" -ForegroundColor Yellow
}

# ── 版本号 ──────────────────────────────────────────────────────────────
$cargoTomlPath = Join-Path $root 'Cargo.toml'
$cargoToml = Get-Content -LiteralPath $cargoTomlPath -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
    throw "无法从 Cargo.toml 读取版本号：$cargoTomlPath"
}
$version = $Matches[1]

# ── 目标平台 ────────────────────────────────────────────────────────────
$hostTriple = (& rustc -vV | Select-String -Pattern '^host:\s*(.+)$').Matches[0].Groups[1].Value.Trim()
if (-not $Target) {
    $Target = $hostTriple
}
if ($Target -notlike '*windows*') {
    throw "本脚本只构建 Windows 产物，当前目标：$Target"
}

switch -Wildcard ($Target) {
    'x86_64-*' { $arch = 'x64' }
    'aarch64-*' { $arch = 'arm64' }
    'i686-*' { $arch = 'x86' }
    default { throw "不认识的目标架构：$Target" }
}

$isHostTarget = ($Target -eq $hostTriple)
$targetArgs = @()
$binDir = Join-Path $root 'target\release'
if (-not $isHostTarget) {
    $targetArgs = @('--target', $Target)
    $binDir = Join-Path $root "target\$Target\release"
}

Write-Step "MNS $version · $Target（$arch）"

# ── 编译 ────────────────────────────────────────────────────────────────
if (-not $SkipBuild) {
    Write-Step '编译 mns.exe（CLI，release）'
    & cargo build --release @targetArgs --bin mns
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --bin mns 失败（exit $LASTEXITCODE）"
    }

    Write-Step '编译 mns-gui.exe（桌面端，release，--features gui）'
    & cargo build --release @targetArgs --features gui --bin mns-gui
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --features gui --bin mns-gui 失败（exit $LASTEXITCODE）"
    }
}
else {
    Write-Step '跳过编译，使用已有构建产物'
}

$guiExe = Join-Path $binDir 'mns-gui.exe'
$cliExe = Join-Path $binDir 'mns.exe'
foreach ($exe in @($guiExe, $cliExe)) {
    if (-not (Test-Path -LiteralPath $exe)) {
        throw "缺少构建产物：$exe（先执行 cargo build --release --features gui --bin mns-gui 与 --bin mns）"
    }
}

# 跨架构产物在本机跑不起来，版本门只对 host 目标执行。
if ($isHostTarget) {
    $reported = (& $cliExe --version | Select-Object -First 1)
    if ($reported -notlike "*$version*") {
        throw "CLI 版本不匹配：期望 $version，实际 '$reported'"
    }
}

# ── 暂存目录 + 便携 ZIP ──────────────────────────────────────────────────
$stageName = "MNS-$version-win-$arch"
$stageDir = Join-Path $distDir $stageName
if (Test-Path -LiteralPath $stageDir) {
    Remove-Item -LiteralPath $stageDir -Recurse -Force
}
New-Item -ItemType Directory -Path $stageDir -Force | Out-Null

Copy-Item -LiteralPath $guiExe -Destination $stageDir
Copy-Item -LiteralPath $cliExe -Destination $stageDir
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination $stageDir

$zipPath = Join-Path $distDir "$stageName.zip"
if (Test-Path -LiteralPath $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
}
Write-Step "打包便携 ZIP：$zipPath"
Compress-Archive -Path (Join-Path $stageDir '*') -DestinationPath $zipPath -CompressionLevel Optimal

# ── 查找 Inno Setup ────────────────────────────────────────────────────
function Resolve-Iscc {
    param([string]$Explicit)

    if ($Explicit) {
        if (Test-Path -LiteralPath $Explicit) {
            return (Resolve-Path -LiteralPath $Explicit).Path
        }
        throw "找不到指定的 ISCC.exe：$Explicit"
    }

    if ($env:ISCC -and (Test-Path -LiteralPath $env:ISCC)) {
        return (Resolve-Path -LiteralPath $env:ISCC).Path
    }

    $command = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $candidates = @()
    if ($env:LOCALAPPDATA) {
        foreach ($flavour in @('Inno Setup 7', 'Inno Setup 6')) {
            $candidates += (Join-Path $env:LOCALAPPDATA "Programs\$flavour\ISCC.exe")
        }
    }
    foreach ($scope in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if (-not $scope) { continue }
        foreach ($flavour in @('Inno Setup 7', 'Inno Setup 6')) {
            $candidates += (Join-Path $scope "$flavour\ISCC.exe")
        }
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }

    # 注册表兜底：兼容 Inno Setup 5/6、任意安装盘符。
    $uninstallRoots = @(
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall'
    )
    foreach ($uninstallRoot in $uninstallRoots) {
        if (-not (Test-Path $uninstallRoot)) { continue }
        $entries = Get-ChildItem $uninstallRoot -ErrorAction SilentlyContinue |
            ForEach-Object { Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue } |
            Where-Object { $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -like 'Inno Setup*' }
        foreach ($entry in $entries) {
            $locations = @()
            if ($entry.PSObject.Properties['InstallLocation']) { $locations += $entry.InstallLocation }
            if ($entry.PSObject.Properties['UninstallString']) {
                $uninstallString = [string]$entry.UninstallString
                if ($uninstallString -match '^\s*"([^"]+\.exe)"') {
                    $locations += (Split-Path -Parent $Matches[1])
                }
                elseif ($uninstallString -match '^\s*(.+?\.exe)(\s|$)') {
                    $locations += (Split-Path -Parent $Matches[1])
                }
            }
            foreach ($location in $locations) {
                if (-not $location) { continue }
                $candidate = Join-Path $location 'ISCC.exe'
                if (Test-Path -LiteralPath $candidate) {
                    return $candidate
                }
            }
        }
    }

    return $null
}

$setupPath = Join-Path $distDir "$stageName-setup.exe"

if ($PortableOnly) {
    Write-Step '按参数要求跳过安装包编译（-PortableOnly）'
}
elseif ($arch -ne 'x64') {
    Write-Warn "Inno Setup 脚本目前只声明 x64，$arch 目标只生成便携 ZIP"
}
else {
    $iscc = Resolve-Iscc -Explicit $InnoSetupPath
    if (-not $iscc) {
        Write-Warn '未找到 Inno Setup 的 ISCC.exe，跳过安装包编译。'
        Write-Host '  安装：winget install -e --id JRSoftware.InnoSetup'
        Write-Host '  或手动安装后重跑本脚本：https://jrsoftware.org/isdl.php'
        if ($RequireInstaller) {
            throw '未找到 Inno Setup（-RequireInstaller 要求必须生成安装包）'
        }
    }
    else {
        $issPath = Join-Path $PSScriptRoot 'installer\mns.iss'
        $iconPath = Join-Path $root 'assets\app.ico'
        $licensePath = Join-Path $root 'LICENSE'
        if (-not (Test-Path -LiteralPath $iconPath)) {
            throw "缺少安装包图标：$iconPath"
        }

        Write-Step "编译安装包（$iscc）"
        & $iscc `
            "/DAppVersion=$version" `
            "/DSourceDir=$stageDir" `
            "/DOutputDir=$distDir" `
            "/DIconFile=$iconPath" `
            "/DLicenseFile=$licensePath" `
            "/DArchName=$arch" `
            $issPath
        if ($LASTEXITCODE -ne 0) {
            throw "Inno Setup 编译失败（exit $LASTEXITCODE）"
        }
        if (-not (Test-Path -LiteralPath $setupPath)) {
            throw "Inno Setup 未产出预期文件：$setupPath"
        }
    }
}

# ── 结果 ────────────────────────────────────────────────────────────────
function Format-Artifact {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path
    $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.Substring(0, 16)
    $sizeMb = [math]::Round($item.Length / 1MB, 1)
    return "   $($item.Name)  $sizeMb MB  sha256:$hash…"
}

Write-Host ''
Write-Host "产物目录：$distDir" -ForegroundColor Green
Write-Host (Format-Artifact -Path $zipPath)
if (Test-Path -LiteralPath $setupPath) {
    Write-Host (Format-Artifact -Path $setupPath)
}
Write-Host ''
Write-Host "   GUI  $guiExe"
Write-Host "   CLI  $cliExe"
Write-Host '   安装后 mns-gui 会自动把同目录的 mns.exe 交给外接 Agent，无需配置 PATH。'
