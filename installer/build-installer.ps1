param()

$ErrorActionPreference = "Stop"
$InstallerProject = "D:\NeoBox\installer"

# Read version from main project's Cargo.toml
$mainCargo = "D:\NeoBox\src-tauri\Cargo.toml"
$version = (Select-String -Path $mainCargo -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
Write-Host "Main app version: $version" -ForegroundColor Cyan

# Build installer
Set-Location $InstallerProject
npm run tauri:build
if ($LASTEXITCODE -ne 0) { throw "Installer build failed" }

# Rename output
$builtExe = "$InstallerProject\src-tauri\target\release\neobox-installer.exe"
$outputName = "NeoBox_${version}_Windows_x86_64.exe"
$outputPath = "$InstallerProject\$outputName"

if (Test-Path $builtExe) {
    Copy-Item $builtExe $outputPath -Force
    $size = (Get-ChildItem $outputPath).Length
    Write-Host "`n=== SUCCESS ===" -ForegroundColor Green
    Write-Host "Output: $outputPath" -ForegroundColor Green
    Write-Host "Size: $('{0:N1}' -f ($size / 1MB)) MB" -ForegroundColor Green
} else {
    Write-Host "`n=== BUILD FAILED ===" -ForegroundColor Red
}
