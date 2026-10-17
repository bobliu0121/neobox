param()

$ErrorActionPreference = "Stop"
$MainProject = "D:\NeoBox"
$InstallerProject = "D:\NeoBox\installer"
$UninstallerProject = "D:\NeoBox\uninstaller"
$PayloadZip = "$InstallerProject\src-tauri\payload.zip"
$TempDir = "$env:TEMP\neobox-payload"

Write-Host "=== Creating payload archive ===" -ForegroundColor Cyan

# Clean temp directory
if (Test-Path $TempDir) {
    Remove-Item "$TempDir\*" -Recurse -Force -ErrorAction SilentlyContinue
}
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

# 1. Main exe
$mainExe = "$MainProject\src-tauri\target\release\neobox.exe"
if (Test-Path $mainExe) {
    Copy-Item $mainExe $TempDir\
    Write-Host "  [OK] neobox.exe"
} else {
    Write-Warning "  [WARN] neobox.exe not found (build main app first)"
}

# 2. Tauri runtime resources
$appResources = "$MainProject\src-tauri\resources"
if (Test-Path $appResources) {
    New-Item -ItemType Directory -Path "$TempDir\resources" -Force | Out-Null
    Copy-Item "$appResources\*" "$TempDir\resources\" -Recurse -Force
    Write-Host "  [OK] resources/*"
}

# 3. Power plans
$powerPlans = "$MainProject\power-plans"
if (Test-Path $powerPlans) {
    New-Item -ItemType Directory -Path "$TempDir\power-plans" -Force | Out-Null
    Copy-Item "$powerPlans\*" "$TempDir\power-plans\" -Recurse -Force
    Write-Host "  [OK] power-plans/*"
}

# 4. Monitor files
$monitorDir = "$MainProject\monitor\bin\Release\net48"
if (Test-Path $monitorDir) {
    New-Item -ItemType Directory -Path "$TempDir\monitor" -Force | Out-Null
    Copy-Item "$monitorDir\*" "$TempDir\monitor\" -Recurse -Force
    Write-Host "  [OK] monitor/*"
}

# 5. AQ registry tweak files (apply)
$aqRegistry = "$MainProject\aq_registry"
if (Test-Path $aqRegistry) {
    New-Item -ItemType Directory -Path "$TempDir\aq_registry" -Force | Out-Null
    Copy-Item "$aqRegistry\*" "$TempDir\aq_registry\" -Recurse -Force
    Write-Host "  [OK] aq_registry/*"
}

# 6. AQ registry tweak files (restore)
$aqRegistryRestore = "$MainProject\aq_registry_restore"
if (Test-Path $aqRegistryRestore) {
    New-Item -ItemType Directory -Path "$TempDir\aq_registry_restore" -Force | Out-Null
    Copy-Item "$aqRegistryRestore\*" "$TempDir\aq_registry_restore\" -Recurse -Force
    Write-Host "  [OK] aq_registry_restore/*"
}

# 7. Flat root files
$rootFiles = @(
    "nvidiaProfileInspector.exe",
    "nvidiaProfileInspector.exe.config",
    "Reference.xml",
    "PawnIO_setup.exe"
)
foreach ($file in $rootFiles) {
    $src = "$MainProject\$file"
    if (Test-Path $src) {
        Copy-Item $src $TempDir\
        Write-Host "  [OK] $file"
    } else {
        Write-Warning "  [WARN] $file not found (skipping)"
    }
}

# 8. NVAPI lib
$nvapiDir = "$TempDir\R560-developer\amd64"
New-Item -ItemType Directory -Path $nvapiDir -Force | Out-Null
$nvapiLib = "$MainProject\R560-developer\amd64\nvapi64.lib"
if (Test-Path $nvapiLib) {
    Copy-Item $nvapiLib $nvapiDir\
    Write-Host "  [OK] R560-developer/amd64/nvapi64.lib"
} else {
    Write-Warning "  [WARN] nvapi64.lib not found (skipping)"
}

# 9. Uninstaller
$uninstExe = "$UninstallerProject\src-tauri\target\release\uninstneobox.exe"
if (Test-Path $uninstExe) {
    Copy-Item $uninstExe $TempDir\
    Write-Host "  [OK] Uninstneobox.exe"
} else {
    Write-Warning "  [WARN] Uninstneobox.exe not found (build uninstaller first)"
}

# 10. Create ZIP
Write-Host "`nCompressing payload..." -ForegroundColor Cyan
if (Test-Path $PayloadZip) {
    Remove-Item $PayloadZip -Force
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($TempDir, $PayloadZip, [System.IO.Compression.CompressionLevel]::Optimal, $false)

$zipSize = (Get-ChildItem $PayloadZip).Length
$fileCount = (Get-ChildItem $TempDir -Recurse -File | Measure-Object).Count
Write-Host "  [DONE] $fileCount files -> payload.zip, $('{0:N1}' -f ($zipSize / 1MB)) MB" -ForegroundColor Green

# Cleanup temp
Remove-Item $TempDir -Recurse -Force -ErrorAction SilentlyContinue
