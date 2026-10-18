# 架构与开发约定

## 1. 总体结构

```text
┌──────────────────────────── React (src/) ────────────────────────────┐
│ main.tsx → providers → App.tsx → MainLayout + Routes                 │
│ 页面 pages/ · 组件 components/ · 上下文 contexts/ · 状态 stores/     │
│ invoke("command") ───────────────────────────────┐                   │
└──────────────────────────────────────────────────┼───────────────────┘
                                                   │ Tauri IPC
┌──────────────────────────── Rust (src-tauri/) ───▼───────────────────┐
│ lib.rs：Builder、插件、setup、托盘、窗口事件、命令注册                 │
│ 业务模块：hardware / optimization / music / wallpaper / ...           │
│ 外部资源：resources/、power-plans/、aq_registry/、monitor/            │
│ 辅助进程：NeoBoxMonitor.exe（C#，stdin/stdout 管道）                  │
└───────────────────────────────────────────────────────────────────────┘
```

- 前端是纯 SPA，无 Next.js/SSR；`vite.config.ts` 提供 `@` → `src` 别名。
- 后端是单 crate `neobox`（lib 名 `neobox_lib`）+ 独立 workspace crate `nx-dl-engine`。
- 前后端只通过 `#[tauri::command]` 与事件通信；没有 HTTP 业务后端（音乐音频代理与本地 HTTP 服务除外）。

## 2. 前端分层

| 目录 | 职责 |
| --- | --- |
| `src/main.tsx` | 挂载 React、Provider 组合、浏览器开发模式下的 Tauri mock |
| `src/App.tsx` | 路由表、多窗口独立页面分支、启动屏/更新/Release 历史/灵动岛宿主 |
| `src/pages/` | 每个功能页一个文件；路由页面默认导出组件 |
| `src/components/` | 可复用 UI；`ui/` 为布局与全局组件，`special/` 为视觉组件 |
| `src/contexts/` | 跨页面状态（主题、字体、背景、音乐、启动流程、更新） |
| `src/stores/` | Zustand 持久化状态（音乐播放器） |
| `src/lib/` | 无 UI 的工具函数与 Tauri 调用封装 |
| `src/config/` | 搜索索引、优化项清单等静态配置 |
| `src/locales/` | zh / zh-TW / en 三套文案；新增文案必须三套同时补齐 |

### 多窗口

`App.tsx` 依据 `location.pathname` 直接渲染独立窗口页面，不套用主布局：

- `/desktop-lyrics`：桌面歌词
- `/vertical-overlay`：竖排悬浮窗
- `/dynamic-island`：桌面灵动岛
- `/sensor-monitor`：传感器监视器
- `/wallpaper-window`：动态壁纸层（GIF / HTML，透明置底窗口，同时由 `wallpaper.rs` 挂到 `WorkerW`）

窗口 label 与权限在 `src-tauri/capabilities/default.json` 中维护；新增窗口必须同时更新该文件。

### 搜索索引

`src/config/search-index.ts` 维护全局搜索的页面与工具条目。新增页面时，如果希望可被搜索，需要补充 `id/nameKey/path/icon/category/keywords`。

## 3. 后端分层

`src-tauri/src/lib.rs` 是唯一的装配入口：

1. 注册插件（single-instance、shell、fs、store、os、global-shortcut、opener、clipboard、dialog）。
2. `setup` 中初始化日志、托盘、传感器子进程、音乐代理、窗口位置等。
3. `invoke_handler(tauri::generate_handler![...])` 注册 400+ 个命令。
4. `app.run` 中处理退出清理。

业务模块按领域拆分，典型职责：

| 模块 | 职责 |
| --- | --- |
| `hardware.rs` / `sensor.rs` / `sensor_monitor.rs` | 硬件信息、传感器、`NeoBoxMonitor` 管道 |
| `optimization.rs` / `storage_scan/` / `storage_clean.rs` | 内存、垃圾、启动项、服务等系统维护 |
| `music.rs` / `music_api/` / `audio_engine.rs` / `smtc.rs` | 多平台音乐、音频播放、系统媒体控制 |
| `wallpaper.rs` / `wallpaper_server.rs` / `video_bg.rs` / `overlay_panel.rs` | 壁纸与 HTML 壁纸本地静态服务、视频背景、悬浮窗 |
| `nvapi.rs` / `display_cache.rs` | NVIDIA 分辨率、显示缓存 |
| `downloader.rs` | 下载与更新 |
| `tray.rs` / `main_window.rs` / `hotkey.rs` | 托盘、主窗口、全局热键 |

### 命令注册

新增命令时：

1. 在对应模块写 `#[tauri::command] pub fn/async fn`。
2. 在 `lib.rs` 的 `generate_handler!` 列表中登记（路径写全：`module::command`）。
3. 前端用 `invoke("command", args)` 调用；命令名默认等于函数名。
4. 涉及文件/窗口/对话框权限时，同步检查 `capabilities/default.json`。

### 资源解析

运行时资源不嵌入 exe，而是放在 exe 同目录或 Tauri `resource_dir()` 下。现有代码同时探测：

- `<resource_dir>/<name>`
- `<resource_dir>/resources/<name>`
- 开发环境下的 `src-tauri/resources/<name>`

因此 `scripts/build-portable.ps1` 必须把 `resources/`、`monitor/`、`power-plans/`、`aq_registry*` 一并放到 exe 旁边。

## 4. 状态与持久化

- 设置项通过 `@tauri-apps/plugin-store` 持久化，键名集中在 `src/lib/store.ts` 与各 context。
- 主题、字体、背景、音乐状态使用 React Context；音乐播放器使用 Zustand。
- 后端配置（CPU 调度规则、内存清理选项等）由 Rust 侧持久化，前端只负责展示与触发。

## 5. 构建标志

`src/lib/build-flags.ts` 中的 `IS_STORE_BUILD` 由 Vite `--mode store` 注入。商店版会隐藏第三方软件获取入口；新增下载/安装类入口时应遵循同一开关。

## 6. 代码约定

- 不提交 `dist/`、`src-tauri/target/`、`artifacts/`、`monitor/bin/`、子项目 `gen/`。
- 新增前端文件必须能从 `main.tsx` 静态或动态导入到达，否则视为死代码。
- 新增静态资源前先确认已有资源不可复用；`public/` 下的文件不会被 Vite 哈希处理，删除前需全仓搜索文件名。
- 文档中的路径、命令、功能名必须与当前源码一致；删除功能时同步删除对应文档与 i18n 文案。
- Rust 侧发布构建应保持零警告；新增 `#[allow(dead_code)]` 需说明原因。

## 7. 已知技术债

- 部分历史页面文件较大（数千行），组件拆分优先级低于行为回归风险，暂不做大规模重写。
- `src/locales/*.json` 仍包含已删除模块的少量文案，后续可按页面逐步收敛。
- `installer/` 与 `uninstaller/` 是独立 Tauri 项目，依赖版本与主项目未完全统一。
