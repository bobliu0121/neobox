# NeoBox

> **原作声明**：本项目源自 [NexBox](https://github.com/MuLiuSaMa/NexBox)（原作作者：MuLiuSaMa，采用 GPL-3.0 协议）。本仓库为在原作基础上进行的定制修改分支：移除了游戏、Steam/Epic、手机远程控制等模块，保留并优化了监控面板与系统工具，原作版权归原作者所有，本分支修改遵循 GPL-3.0 许可。

NeoBox 是一款 Windows 桌面工具箱，基于 Tauri 2 + React 构建，纯本地运行、低资源占用，把常用的系统监控与维护工具整合进一个界面。

## 功能

| 模块 | 说明 |
| --- | --- |
| 硬件监控面板 | 实时监控 CPU / GPU / 内存 / 温度 / 风扇等传感器数据 |
| 系统优化 | 垃圾清理、网络修复、启动项管理等系统维护工具 |
| 音乐中心 | 网易云 / 酷狗 / QQ 音乐 / 咪咕 / 汽水 / Spotify 多平台搜索与播放；未登录也可匿名搜索试听，VIP 歌曲带标识 |
| 图片格式转换 | png / jpg / jpeg / webp / bmp 之间相互转换 |
| 音频格式转换 | wav / mp3 / m4a 之间相互转换 |
| 内置工具 | 显卡驱动管理、运行库修复等实用工具 |

## 技术栈

- Tauri 2（Rust 后端）
- React 19 + Chakra UI
- TypeScript / Vite
- 多语言：简体中文 / 繁体中文 / English / Deutsch / Français / 日本語

## 构建

```bash
# 前端
npm install
npm run build

# 后端（需 Rust 工具链）
cd src-tauri
cargo build --release --features custom-protocol
```

## 问题反馈

- GitHub Issues：https://github.com/bobliu0121/neobox/issues

## 许可证

[GPL-3.0](LICENSE)

原作：[NexBox](https://github.com/MuLiuSaMa/NexBox)（MuLiuSaMa）
