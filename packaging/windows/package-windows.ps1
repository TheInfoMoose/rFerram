# rFerram Windows Release Packaging Script (.zip and .msi)
# Usage: pwsh packaging/windows/package-windows.ps1 -Version <ver> -Target <triple> [-OutDir <dir>]
param(
    [string]$Version = "0.1.0",
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"

$CleanVersion = $Version -replace '^v', ''
$MsiVersion = $CleanVersion -replace '-.*$', ''
if ($MsiVersion -notmatch '^\d+\.\d+\.\d+(\.\d+)?$') {
    $MsiVersion = "0.1.0"
}

$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $WorkspaceRoot

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDirPath = (Resolve-Path $OutDir).Path

$TargetBinDir = Join-Path $WorkspaceRoot "target\$Target\release"
$DefaultBinDir = Join-Path $WorkspaceRoot "target\release"

if (Test-Path (Join-Path $TargetBinDir "rferram-ui.exe")) {
    $BinDir = $TargetBinDir
} elseif (Test-Path (Join-Path $DefaultBinDir "rferram-ui.exe")) {
    $BinDir = $DefaultBinDir
} else {
    throw "Compiled binaries (rferram-ui.exe / rferram-server.exe) not found in $TargetBinDir or $DefaultBinDir"
}

if ($Target -like "aarch64-*") {
    $WixArch = "arm64"
} else {
    $WixArch = "x64"
}

# ---------------------------------------------------------------------------
# 1. Portable Archive (.zip)
# ---------------------------------------------------------------------------
$StageDir = Join-Path ([System.IO.Path]::GetTempPath()) "rferram-$CleanVersion-$Target"
if (Test-Path $StageDir) {
    Remove-Item -Recurse -Force $StageDir
}
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

Copy-Item (Join-Path $BinDir "rferram-ui.exe") -Destination $StageDir
Copy-Item (Join-Path $BinDir "rferram-server.exe") -Destination $StageDir
Copy-Item (Join-Path $WorkspaceRoot "README.md") -Destination $StageDir
Copy-Item (Join-Path $WorkspaceRoot "LICENSE") -Destination $StageDir

$ZipFile = Join-Path $OutDirPath "rferram-$CleanVersion-$Target.zip"
if (Test-Path $ZipFile) {
    Remove-Item -Force $ZipFile
}
Compress-Archive -Path "$StageDir\*" -DestinationPath $ZipFile
Write-Host "==> Created portable archive: $ZipFile"

Remove-Item -Recurse -Force $StageDir

# ---------------------------------------------------------------------------
# 2. Windows Installer Package (.msi via WiX v5)
# ---------------------------------------------------------------------------
if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    Write-Host "==> Installing WiX Toolset CLI via dotnet tool..."
    dotnet tool install --global wix --version 5.0.2
    $DotnetTools = Join-Path $env:USERPROFILE ".dotnet\tools"
    if ($env:PATH -notlike "*$DotnetTools*") {
        $env:PATH = "$DotnetTools;$env:PATH"
    }
}

$MsiFile = Join-Path $OutDirPath "rferram-$CleanVersion-$Target.msi"
$WxsFile = Join-Path $WorkspaceRoot "packaging\windows\rferram.wxs"

wix build $WxsFile `
    -arch $WixArch `
    -d "Version=$MsiVersion" `
    -d "BinDir=$BinDir" `
    -d "RootDir=$WorkspaceRoot" `
    -o $MsiFile

if ($LASTEXITCODE -ne 0) {
    throw "wix build failed with exit code $LASTEXITCODE"
}
Write-Host "==> Created Windows MSI installer: $MsiFile"
