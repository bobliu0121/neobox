# NeoBox

> **原作声明**：本项目源自 [NexBox](https://github.com/MuLiuSaMa/NexBox)（原作作者：MuLiuSaMa，采用 GPL-3.0 协议）。本仓库为在原作基础上进行的定制修改分支：移除了游戏库、Steam/Epic、手机远程控制等模块，保留并持续维护硬件监控、系统优化与桌面工具，原作版权归原作者所有，本分支修改遵循 GPL-3.0 许可。

NeoBox 是一款 Windows 桌面工具箱，基于 Tauri 2 + React 构建，纯本地运行，把常用的系统监控、维护与桌面工具整合进一个界面。

## 功能

| 模块 | 说明 |
| --- | --- |
| 硬件监控 | CPU / GPU / 内存 / 磁盘 / 温度 / 风扇等传感器数据；悬浮窗、桌面歌词、灵动岛、竖排悬浮窗 |
| 系统优化 | 内存清理、启动项、服务、右键菜单、电源计划、网络优化、Windows 更新、CPU 调度、隐藏功能 |
| 存储与文件 | 垃圾清理、大文件扫描、磁盘健康（SMART）、图片/音频格式转换、下载加速 |
| 外设与显示 | 分辨率管理、屏幕检测、NVIDIA 录制、壁纸管理；GIF / HTML 动态壁纸（BETA） |
| 音乐中心 | 网易云 / 酷狗 / QQ 音乐 / 咪咕 / 汽水 / Spotify 搜索与播放；未登录也可匿名搜索试听 |
| 工具与扩展 | 运行库修复、驱动安装、蓝屏日志、时间校准、网络测速、应用管理 |

## 技术栈

- Tauri 2（Rust 后端）
- React 19 + Chakra UI + Vite 6
- TypeScript
- C# / .NET Framework 4.8（`monitor/` 硬件传感器辅助进程）
- 多语言：简体中文 / 繁体中文 / English

## 目录结构

```text
src/                 前端 React 应用（页面、组件、上下文、i18n）
src-tauri/           Tauri/Rust 后端与随包资源
  src/               后端模块（400+ 个 IPC 命令）
  nx-dl-engine/      独立下载引擎 crate
  resources/         运行时资源（驱动工具、ICC、词典等）
monitor/             C# 传感器监视器（NeoBoxMonitor.exe）
public/              前端静态资源（字体、图标、预览图）
installer/           安装程序（独立 Tauri 子项目）
uninstaller/         卸载程序（独立 Tauri 子项目）
msix/                MSIX 打包与侧载脚本
docs/                构建、架构与功能说明
scripts/             本地开发/发布辅助脚本
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

需要可直接运行的便携目录时：

```powershell
npm run build:portable
# 输出：artifacts\NeoBox-<version>-portable\
```

更完整的依赖说明、Windows GNU 工具链注意事项和打包流程见 [docs/BUILD.md](docs/BUILD.md)；目录职责与新增页面/命令的约定见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 文档

- [构建与打包](docs/BUILD.md)
- [架构与开发约定](docs/ARCHITECTURE.md)
- [功能文档索引](docs/README.md)
- [壁纸与动态壁纸](docs/wallpaper.md)

## 问题反馈

- GitHub Issues：https://github.com/bobliu0121/neobox/issues

## 许可证

[GPL-3.0](LICENSE)

原作：[NexBox](https://github.com/MuLiuSaMa/NexBox)（MuLiuSaMa）
