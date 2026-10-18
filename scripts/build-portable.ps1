# ============================================================
# NeoBox 便携版构建脚本
# 用途：一条命令完成 前端 -> 传感器监视器 -> Rust release -> 资源归集
# 产物：artifacts\NeoBox-<version>-portable\
# 说明：tauri.conf.json 的 bundle.active=false，默认 cargo build 不会生成安装包，
#       因此这里按运行时约定把 exe 与外部资源放到同一目录。
# ============================================================
param(
    [string]$OutputRoot = "",
    [switch]$SkipFrontend,
    [switch]$SkipMonitor
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $OutputRoot = Join-Path $Root "artifacts"
}
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)

function Invoke-Step {
    param([string]$Name, [scriptblock]$Action)
    Write-Host "`n=== $Name ===" -ForegroundColor Cyan
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Name 失败 (exit code $LASTEXITCODE)"
    }
}

if (-not (Get-Command npm.cmd -ErrorAction SilentlyContinue)) {
    throw "未找到 npm.cmd，请先安装 Node.js 22+ 并加入 PATH"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "未找到 cargo，请先安装 Rust 工具链并加入 PATH"
}

$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
if ($cargoHome -match '[^\x00-\x7F]') {
    Write-Warning "CARGO_HOME 含非 ASCII 字符：$cargoHome"
    Write-Warning "Windows GNU 工具链下 ring 的 ar 步骤可能失败，建议先设置 CARGO_HOME 到纯 ASCII 目录。"
}
if ($Root -match '[^\x00-\x7F]') {
    Write-Warning "仓库路径含非 ASCII 字符：$Root"
    Write-Warning "Windows GNU 工具链的 windres 可能无法读取 icons/icon.ico，建议在 C:\nexbox-src 等 ASCII 路径下构建。"
}

# Windows PowerShell 5.1 默认按 ANSI 读取文件，tauri.conf.json 含中文标题，必须显式 UTF-8
$version = (Get-Content (Join-Path $Root "src-tauri\tauri.conf.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version
$stage = Join-Path $OutputRoot "NeoBox-$version-portable"

if (-not $SkipFrontend) {
    Invoke-Step "构建前端 (tsc + vite)" {
        Push-Location $Root
        try { & npm.cmd run build } finally { Pop-Location }
    }
}

if (-not $SkipMonitor) {
    if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
        throw "未找到 dotnet，请安装 .NET SDK 8+ 以构建 NeoBoxMonitor"
    }
    Invoke-Step "构建传感器监视器 (.NET Framework 4.8)" {
        Push-Location $Root
        try { & dotnet build "monitor\NeoBoxMonitor.csproj" -c Release --nologo } finally { Pop-Location }
    }
}

Invoke-Step "构建 Rust 后端 (release + custom-protocol)" {
    Push-Location (Join-Path $Root "src-tauri")
    try { & cargo build --release --features custom-protocol } finally { Pop-Location }
}

$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root "src-tauri\target" }
$exe = Join-Path $targetRoot "release\neobox.exe"
if (-not (Test-Path $exe)) {
    throw "未找到构建产物：$exe"
}

# 只清理本脚本自己的输出目录，避免误删其它构建结果
$outputPrefix = $OutputRoot.TrimEnd('\') + '\'
if (-not $stage.StartsWith($outputPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "拒绝清理非输出目录路径：$stage"
}
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

Copy-Item $exe $stage
Copy-Item (Join-Path $Root "WebView2Loader.dll") $stage

$resourceCopies = @(
    @{ Source = "src-tauri\resources"; Target = "resources" },
    @{ Source = "monitor\bin\Release\net48"; Target = "monitor" },
    @{ Source = "power-plans"; Target = "power-plans" },
    @{ Source = "aq_registry"; Target = "aq_registry" },
    @{ Source = "aq_registry_restore"; Target = "aq_registry_restore" }
)
foreach ($item in $resourceCopies) {
    $source = Join-Path $Root $item.Source
    if (-not (Test-Path $source)) {
        throw "缺少运行时资源目录：$source"
    }
    $target = Join-Path $stage $item.Target
    New-Item -ItemType Directory -Path $target -Force | Out-Null
    Copy-Item (Join-Path $source "*") $target -Recurse -Force
}

$stats = Get-ChildItem $stage -Recurse -File | Measure-Object Length -Sum
Write-Host "`n=== 构建完成 ===" -ForegroundColor Green
Write-Host "输出目录：$stage"
Write-Host ("文件数：{0}，总大小：{1:N1} MB" -f $stats.Count, ($stats.Sum / 1MB))
Write-Host "主程序：$stage\neobox.exe"
