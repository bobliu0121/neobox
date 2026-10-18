# 壁纸与动态壁纸

壁纸页面路由：`/wallpaper`（`src/pages/WallpaperPage.tsx`）。

## 功能范围

| 能力 | 实现方式 | 状态 |
| --- | --- | --- |
| PNG / JPG / WEBP / BMP / TIFF 等静态图片 | Windows `SystemParametersInfoW(SPI_SETDESKWALLPAPER)` | 稳定 |
| 多文件夹导入（单层 / 含子文件夹）、历史记录、轮换播放、循环播放列表、图片编辑器 | 前端状态 + `settings.json` 持久化 | 稳定 |
| GIF 动态壁纸 | 桌面壁纸层透明 WebView2 窗口 + `<img>` 播放 | **BETA** |
| HTML 动态壁纸 | 桌面壁纸层透明 WebView2 窗口直接导航本地网页 | **BETA** |

动态壁纸在 UI 中以 `BETA` 徽标标注，表示可能存在个别机器/显卡驱动下的兼容性问题。

## 壁纸文件夹持久化

「壁纸管理」提供两个导入按钮：

- **导入文件夹（仅当前层）**：只扫描所选目录第一层的图片 / HTML；
- **导入文件夹（含子文件夹）**：递归扫描所有子目录。

导入的文件夹（路径 + 是否递归）写入 `settings.json` 的 `wallpaper_folders`。
进入页面时自动重新扫描这些文件夹，因此关闭应用后再次打开，壁纸列表与文件夹标签
仍然存在；点击「刷新」也会按当前模式重新扫描。移除文件夹标签会同时从持久化列表
中删除。

## 动态壁纸实现

后端：`src-tauri/src/wallpaper.rs`；前端壁纸窗口页面：`src/pages/WallpaperWindowPage.tsx`。

1. `set_dynamic_wallpaper(path)` 根据扩展名判定类型：
   - `.gif` → 加载应用内路由 `/wallpaper-window`，页面用 `<img>` 以 `object-fit: cover`
     铺满整块屏幕；
   - `.html` / `.htm` → 在 HTML 所在目录启动一个只监听 `127.0.0.1` 的极简静态
     文件服务器（随机端口），页面用全屏 `<iframe>` 加载
     `http://127.0.0.1:<port>/<文件名>`。这样相对路径的 CSS / JS / 图片、ES module、
     `fetch`、CDN 资源都按浏览器语义正常工作。
2. 为什么不用 asset 协议加载 HTML：Tauri 会把整条 Windows 路径 percent-encode 成
   一个 URL 段（`http://asset.localhost/C%3A%5Cdir%5Cindex.html`），浏览器解析
   `style.css` 时会把最后一段整个替换掉，得到 `http://asset.localhost/style.css`，
   页面的样式/脚本全部 404。也不能让窗口顶层导航到 asset URL —— WebView2 的
   `WebResourceRequested` 不覆盖顶层文档导航，页面会直接空白。
3. 窗口参数：透明、无边框、不可缩放、`skip_taskbar`、不抢焦点；
   创建后追加 `WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` 实现鼠标穿透。
4. 挂载到桌面壁纸层：向 `Progman` 发送 `0x052C` 让 Explorer 创建承载壁纸的
   `WorkerW`，再 `SetParent` 到该 `WorkerW`（位于桌面图标之下）。Windows 11 24H2
   的 `WorkerW` 是 `Progman` 的子窗口，会优先使用；找不到时回退到 `Progman`，
   并插到 `SHELLDLL_DefView` 之后，仍然位于图标下方。
   作为子窗口前会补上 `WS_CHILD` 并去掉 `WS_POPUP`，否则 z-order 仍按顶级窗口计算。
5. 窗口按宿主客户区尺寸铺满；守护线程每 5 秒检查一次父窗口，Explorer 重启导致
   掉出壁纸层时自动重新挂载。
6. 内容切换复用同一个窗口：页面始终停留在 `/wallpaper-window`，通过
   `wallpaper:changed` 事件切换 `<img>` / `<iframe>`，不会整页重载。
7. 页面在内容加载完成后调用 `wallpaper_window_ready`，窗口才显示并挂载，
   避免透明窗口先闪一帧空白。
8. 退出时 `stop_dynamic_wallpaper` 销毁窗口并停止本地服务；选择静态图片时也会
   先停止动态壁纸。

> 命令实现注意：创建窗口的 IPC 命令必须是 `async fn`。同步命令在主线程执行，
> 而 `WebviewWindowBuilder::build()` 需要主线程事件循环，会直接死锁（桌面无变化、
> 所有 IPC 一起卡住）。

### 持久化与恢复

动态壁纸记录写入 `settings.json` 的 `wallpaper_dynamic_active`（`{ kind, path }`）。
主窗口启动 1.5 秒后读取该记录并自动恢复，因此重启应用不会丢失动态壁纸。

### 全窗口 WebView2 参数

所有动态创建的窗口共用 `src-tauri/src/webview_args.rs` 的 `BROWSER_ARGS`
（WebView2 环境参数必须全窗口一致，否则新窗口创建失败）。其中关闭
`CalculateNativeWinOcclusion` 很关键：置底窗口会被 Chromium 误判为“被遮挡”而暂停渲染，
表现为 GIF 卡住或掉帧。

## 已知限制

- 动态壁纸挂在 Explorer 的桌面壁纸层上，覆盖宿主 `WorkerW` 的客户区；多显示器且
  各屏独立壁纸时，可能只在主屏显示。
- HTML 壁纸会执行页面内的脚本（仅限用户主动选择的本地文件），请勿使用来源不明的网页。
  HTML 文件所在目录会通过 `127.0.0.1` 随机端口的静态服务暴露给壁纸页面，服务在
  切换/停止壁纸后立即关闭，且只允许读取该目录内的文件。
- 如果 HTML 依赖 `file://` 绝对路径或需要访问目录外文件，会被安全校验拒绝。
- 以 BETA 标注：不同 Windows 版本/Explorer 实现、远程桌面、锁屏恢复后可能需要重新应用。

## 排查

| 现象 | 排查方向 |
| --- | --- |
| 桌面全黑/无变化 | 确认文件存在；查看 `%LOCALAPPDATA%\NeoBox\neobox.log` 中 `[Wallpaper]` 日志；确认 Explorer 正常运行 |
| GIF 不动 | 检查是否被其它全屏/置顶窗口遮挡；确认 `BROWSER_ARGS` 未被改动 |
| 壁纸盖住桌面图标 | `WorkerW` 挂载失败，回退路径异常；查看 `[Wallpaper] 已挂载到桌面壁纸层` 日志 |
| HTML 无样式/白底 | 查看本地服务是否启动（日志里的 `HTML 本地服务`）；确认 CSS 路径相对 HTML 文件 |
| HTML 空白 | 确认文件编码为 UTF-8；页面脚本报错可在 `%LOCALAPPDATA%\NeoBox\neobox.log` 附近排查 |
