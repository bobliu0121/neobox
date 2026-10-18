param(
    # 主工程根目录；默认取当前脚本上一级，可在 CI 中显式传入
    [string]$MainProject = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = "Stop"
$InstallerProject = $PSScriptRoot
$UninstallerProject = Join-Path $MainProject "uninstaller"
$PayloadZip = Join-Path $InstallerProject "src-tauri\payload.zip"
$TempDir = Join-Path $env:TEMP "neobox-payload"

Write-Host "=== Creating payload archive ===" -ForegroundColor Cyan

if (Test-Path $TempDir) {
    Remove-Item "$TempDir\*" -Recurse -Force -ErrorAction SilentlyContinue
}
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

function Copy-PayloadDir {
    param([string]$Source, [string]$Target, [string]$Label)
    if (-not (Test-Path $Source)) {
        Write-Warning "  [WARN] $Label not found: $Source"
        return
    }
    New-Item -ItemType Directory -Path (Join-Path $TempDir $Target) -Force | Out-Null
    Copy-Item (Join-Path $Source "*") (Join-Path $TempDir $Target) -Recurse -Force
    Write-Host "  [OK] $Label"
}

# 1. Main exe（必须先构建主程序）
$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $MainProject "src-tauri\target" }
$mainExe = Join-Path $targetRoot "release\neobox.exe"
if (-not (Test-Path $mainExe)) {
    throw "未找到主程序：$mainExe；请先执行 npm run build:portable 或 cargo build --release"
}
Copy-Item $mainExe $TempDir
Copy-Item (Join-Path $MainProject "WebView2Loader.dll") $TempDir
Write-Host "  [OK] neobox.exe / WebView2Loader.dll"

# 2. Runtime resources
Copy-PayloadDir (Join-Path $MainProject "src-tauri\resources") "resources" "resources/*"
Copy-PayloadDir (Join-Path $MainProject "monitor\bin\Release\net48") "monitor" "monitor/*"
Copy-PayloadDir (Join-Path $MainProject "power-plans") "power-plans" "power-plans/*"
Copy-PayloadDir (Join-Path $MainProject "aq_registry") "aq_registry" "aq_registry/*"
Copy-PayloadDir (Join-Path $MainProject "aq_registry_restore") "aq_registry_restore" "aq_registry_restore/*"

# 3. Uninstaller
$uninstTarget = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $UninstallerProject "src-tauri\target" }
$uninstExe = Join-Path $uninstTarget "release\uninstneobox.exe"
if (Test-Path $uninstExe) {
    Copy-Item $uninstExe $TempDir
    Write-Host "  [OK] uninstneobox.exe"
} else {
    Write-Warning "  [WARN] uninstneobox.exe not found; build uninstaller first"
}

# 4. Create ZIP
Write-Host "`nCompressing payload..." -ForegroundColor Cyan
if (Test-Path $PayloadZip) {
    Remove-Item $PayloadZip -Force
}
$payloadDir = Split-Path -Parent $PayloadZip
New-Item -ItemType Directory -Path $payloadDir -Force | Out-Null

Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($TempDir, $PayloadZip, [System.IO.Compression.CompressionLevel]::Optimal, $false)

$zipSize = (Get-Item $PayloadZip).Length
$fileCount = (Get-ChildItem $TempDir -Recurse -File | Measure-Object).Count
Write-Host "  [DONE] $fileCount files -> payload.zip, $('{0:N1}' -f ($zipSize / 1MB)) MB" -ForegroundColor Green

Remove-Item $TempDir -Recurse -Force -ErrorAction SilentlyContinue
