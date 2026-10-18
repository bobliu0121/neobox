# 构建与打包

本文档描述 NeoBox 主程序、传感器监视器和安装/卸载程序的本地构建流程。命令默认在仓库根目录执行。

## 1. 环境要求

| 组件 | 版本 | 用途 |
| --- | --- | --- |
| Node.js | 22+（含 npm） | 前端构建、Tauri CLI |
| Rust | 1.77.2+ | 主程序、安装器、卸载器 |
| .NET SDK | 8+ | 构建 `monitor/NeoBoxMonitor.csproj`（net48） |
| WebView2 Runtime | Windows 10/11 自带或单独安装 | 运行 Tauri 窗口 |
| Windows SDK | 可选 | 仅 MSIX 打包需要 `makeappx` / `signtool` |

`monitor/` 的目标框架是 .NET Framework 4.8。若 `dotnet build` 报缺少 targeting pack，请安装 .NET Framework 4.8 Developer Pack，或安装 Visual Studio Build Tools 的“.NET Framework 4.8 SDK”组件。

## 2. 前端

```bash
npm ci
npm run lint      # ESLint（Vite/React 配置）
npm run build     # tsc 类型检查 + vite 生产构建，输出 dist/
```

`npm run build:store` 使用 store 模式构建（`src/lib/build-flags.ts` 中的 `IS_STORE_BUILD` 会隐藏第三方软件获取入口）。商店版请使用该命令。

## 3. 传感器监视器

```bash
npm run build:monitor
# 等价于：dotnet build monitor/NeoBoxMonitor.csproj -c Release
```

输出目录：`monitor/bin/Release/net48/`。主程序运行时会按以下顺序查找 `NeoBoxMonitor.exe`：

1. `<exe目录>/monitor/`
2. `<exe目录>/NeoBox/monitor/`
3. `<exe目录>/resources/monitor/`
4. `<exe目录>/monitor/bin/Release/net48/`
5. 开发环境下向上回溯项目根目录的 `monitor/bin/Release/net48/`

## 4. Rust 后端

```bash
cd src-tauri
cargo build --release --features custom-protocol
```

- **必须带 `--features custom-protocol`**：否则 release 版仍会使用 `devUrl`，启动后页面空白。
- 默认产物：`src-tauri/target/release/neobox.exe`。
- 设置 `CARGO_TARGET_DIR` 时，产物位于 `$CARGO_TARGET_DIR/release/neobox.exe`。

### Windows GNU 工具链注意（重要）

本仓库当前可在 MSYS2 `x86_64-pc-windows-gnu` 工具链下构建，但 GNU 工具链对非 ASCII 路径支持不完整，需要同时满足：

- **源码目录**使用纯 ASCII 路径。`tauri-winres` 调用 `windres` 编译 `src-tauri/icons/icon.ico` 时，如果仓库位于含中文的路径，会报 `windres: can't open icon file ...: Invalid argument`；此时请把仓库复制/克隆到 `C:\nexbox-src` 等 ASCII 目录构建。
- **CARGO_HOME** 使用纯 ASCII 路径。`ring` 的构建脚本会调用 `ar` 归档预编译对象；如果 `CARGO_HOME` 位于含中文的路径（例如 `C:\Users\明轩\.cargo`），`ar` 可能无法打开其中的 `.o` 文件，报：

```text
ar: ...ring...pregenerated...o: No such file or directory
error: failed to run custom build command for `ring`
```

解决方式：把 `CARGO_HOME` 指向纯 ASCII 目录（该目录会被自动初始化）：

```powershell
$env:CARGO_HOME = "C:\cargo-home"
$env:CARGO_TARGET_DIR = "C:\nexbox-target"
```

或在 Windows 上使用 MSVC 工具链（`rustup default stable-x86_64-pc-windows-msvc`）。

### GNU 构建的运行时 DLL

GNU 产物会动态依赖 `WebView2Loader.dll`。发布时必须把仓库根目录的 `WebView2Loader.dll` 放在 `neobox.exe` 同目录，否则程序启动失败。

## 5. 便携版（推荐用于本地分发）

```powershell
npm run build:portable
# 或
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-portable.ps1
```

脚本依次执行前端、监视器、Rust 构建，并把运行时文件归集到：

```text
artifacts/NeoBox-<version>-portable/
  neobox.exe
  WebView2Loader.dll
  monitor/                 # NeoBoxMonitor.exe 及其依赖
  resources/               # src-tauri/resources
  power-plans/
  aq_registry/
  aq_registry_restore/
```

`tauri.conf.json` 中 `bundle.active=false`，因此普通 `cargo build` 不会生成安装包，也不会自动把外部资源放到 exe 旁边；便携脚本负责这一步。

可用参数：`-SkipFrontend`、`-SkipMonitor`、`-OutputRoot <目录>`。

## 6. 安装程序 / 卸载程序 / MSIX

安装程序与卸载程序是独立的 Tauri 子项目：

```powershell
# 安装程序（先构建主程序）
installer\build-installer.ps1

# 生成安装器 payload（主程序 + 资源 + 卸载器）
installer\pack-payload.ps1

# MSIX（需要 Windows SDK；先执行 pack-payload）
msix\build-msix.ps1
msix\install-msix.ps1
```

所有脚本均以 `$PSScriptRoot` 推导仓库根目录，不再依赖固定的 `D:\NeoBox` 路径。

## 7. 常见问题

| 现象 | 原因 / 处理 |
| --- | --- |
| release 启动白屏、访问 127.0.0.1 | 漏加 `--features custom-protocol` |
| 启动提示缺少 `WebView2Loader.dll` | 未把根目录 DLL 放到 exe 同目录 |
| `ring` 构建失败、`ar` 找不到 `.o` | `CARGO_HOME` 含非 ASCII 路径，改用 ASCII 目录或 MSVC |
| 无硬件温度/风扇数据 | 未构建或未放置 `NeoBoxMonitor.exe`，检查第 3 节目的查找路径 |
| MSIX 打包报缺少 makeappx | 安装 Windows SDK，或传入可用的 SDK bin 路径 |
| 前端 lint 失败 | 先 `npm ci`，确保使用仓库内的 ESLint 9 + typescript-eslint 配置 |

## 8. CI

`.github/workflows/ci.yml` 包含两个作业：

- `web`：`npm ci` → `npm run lint` → `npm run build`
- `tauri`：`npm ci` → 前端构建 → `dotnet build monitor` → `cargo check --features custom-protocol`
