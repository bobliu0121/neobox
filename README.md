# NeoBox

> Windows 桌面工具箱 · 硬件监控 / 系统优化 / 桌面工具<br>
> 当前版本：**v1.0.0** · 作者：**Bobliu** · 基于 [NexBox](https://github.com/MuLiuSaMa/NexBox) 定制维护

NeoBox 是一个基于 **Tauri 2 + React 19** 的 Windows 桌面工具箱。所有功能均在本地运行，把常用的系统监控、维护和桌面增强能力整合到一个轻量界面里，不依赖云端账号。

- 作者：[Bobliu](https://github.com/bobliu0121)
- 仓库：[bobliu0121/neobox](https://github.com/bobliu0121/neobox)
- 问题反馈：[GitHub Issues](https://github.com/bobliu0121/neobox/issues)
- 作者 QQ：3954720009

## 功能

| 模块 | 说明 |
| --- | --- |
| 硬件监控 | CPU / GPU / 内存 / 磁盘 / 温度 / 风扇等传感器数据；悬浮窗、桌面歌词、灵动岛、竖排悬浮窗 |
| 系统优化 | 内存清理、启动项、服务、右键菜单、电源计划、网络优化、Windows 更新、CPU 调度、隐藏功能 |
| 存储与文件 | 垃圾清理、大文件扫描、磁盘健康（SMART）、图片 / 音频格式转换、下载加速 |
| 外设与显示 | 分辨率管理、屏幕检测、NVIDIA 录制、壁纸管理；GIF / HTML 动态壁纸（BETA） |
| 音乐中心 | 网易云 / 酷狗 / QQ 音乐 / 咪咕 / 汽水 / Spotify 搜索与播放；未登录也可匿名搜索试听 |
| 工具与扩展 | 运行库修复、驱动安装、蓝屏日志、时间校准、网络测速、应用管理 |
| 文件搜索 | 类 Everything 的本地文件名即时搜索：MFT 索引、整盘 / 指定目录、按格式筛选 |

动态壁纸中的 **GIF / HTML** 两种模式仍处于 BETA：GIF 通过置底透明窗口挂载到桌面壁纸层，HTML 由本地回环静态服务器提供页面与相对资源。多显示器、混合 DPI 等场景可能只覆盖主屏，详见 [壁纸文档](docs/wallpaper.md)。

## 下载

前往 [Releases](https://github.com/bobliu0121/neobox/releases) 下载最新便携版压缩包：

1. 解压到任意目录（路径建议使用纯英文，避免部分工具读取中文路径失败）。
2. 双击 `neobox.exe` 启动，按提示授予管理员权限。
3. 首次启动会在同目录创建配置与缓存，卸载时直接删除整个目录即可。

系统要求：Windows 10 1809+ / Windows 11（x64），已安装 **WebView2 Runtime**（Windows 11 与新版 Windows 10 通常自带）。

## 技术栈

- Tauri 2（Rust 后端，400+ IPC 命令）
- React 19 + Chakra UI + Vite 6
- TypeScript
- C# / .NET Framework 4.8（`monitor/` 硬件传感器辅助进程）
- 多语言：简体中文 / 繁体中文 / English

## 目录结构

```text
src/                 前端 React 应用（页面、组件、上下文、i18n）
src-tauri/           Tauri / Rust 后端与随包资源
  src/               后端模块
  nx-dl-engine/      独立下载引擎 crate
  resources/         运行时资源（驱动工具、词典等）
monitor/             C# 传感器监视器（NeoBoxMonitor.exe）
public/              前端静态资源（字体、图标、头像、预览图）
installer/           安装程序（独立 Tauri 子项目）
uninstaller/         卸载程序（独立 Tauri 子项目）
msix/                MSIX 打包与侧载脚本
docs/                构建、架构与功能说明
scripts/             本地开发 / 发布辅助脚本
```

## 开发与构建

环境要求：Node.js 22+、Rust 1.77.2+、.NET SDK 8+（构建 `monitor/`）、WebView2 Runtime。

```bash
npm ci
npm run dev                 # 前端开发服务器（浏览器中带 Tauri mock）

# 完整发布构建
npm run build               # tsc + vite build
npm run build:monitor       # 构建 NeoBoxMonitor.exe
cd src-tauri
cargo build --release --features custom-protocol
```

生成可直接运行的便携目录：

```powershell
npm run build:portable
# 输出：artifacts\NeoBox-<version>-portable\
```

> Windows GNU 工具链下，源码路径与 `CARGO_HOME` 建议使用纯 ASCII 目录（例如 `C:\nexbox-src`、`C:\cargo-home`），否则 `windres` / `ring` 可能因中文路径失败。

更完整的依赖说明、工具链注意事项与打包流程见 [docs/BUILD.md](docs/BUILD.md)；目录职责与新增页面 / 命令的约定见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 更新

应用启动后会检查 [GitHub Releases](https://github.com/bobliu0121/neobox/releases) 中的最新版本。当前正式版以便携压缩包发布，检测到新版本时会提示并打开 Release 页面；后续若提供安装包资产，应用内更新会直接下载并引导安装。

## 文档

- [构建与打包](docs/BUILD.md)
- [架构与开发约定](docs/ARCHITECTURE.md)
- [功能文档索引](docs/README.md)
- [壁纸与动态壁纸](docs/wallpaper.md)
- [文件搜索](docs/search.md)

## 致谢与许可

本项目源自 [NexBox](https://github.com/MuLiuSaMa/NexBox)（作者：MuLiuSaMa），遵循 **GPL-3.0** 协议。本分支移除了游戏库、Steam / Epic、手机远程控制等模块，保留并持续维护硬件监控、系统优化与桌面工具。原作版权归原作者所有，本分支修改同样以 GPL-3.0 发布。

许可证：[GPL-3.0](LICENSE)
