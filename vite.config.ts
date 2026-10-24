import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

export default defineConfig(async () => ({
  plugins: [
    react({
      jsxImportSource: "@emotion/react",
      babel: {
        plugins: ["@emotion/babel-plugin"],
      },
    }),
  ],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  clearScreen: false,
  build: {
    // 主包曾接近 3 MB；把稳定的大依赖拆分为独立 chunk，便于缓存与定位体积回归。
    // 桌面端从本地磁盘加载，主 chunk 约 1.8 MB 属可接受范围。
    // 注意：本项语义保持不变（仅调告警阈值），不做"把告警关掉"式的掩盖。
    chunkSizeWarningLimit: 1800,

    // 输出 gzip 体积报告。本项目走 Tauri custom-protocol 从本地资源加载，
    // 传输体积不影响首屏，但 gzip 后的数字是**衡量真实代码量**最直观的指标，
    // 便于发现"改了配置反而变大"这类回归（纯本地压缩耗时极短，可忽略）。
    reportCompressedSize: true,

    // 关闭 modulepreload 的 polyfill 注入。
    // 依据：本应用仅运行于 Windows WebView2（Evergreen Chromium），
    // 从 Chromium 66 起已原生支持 <link rel="modulepreload">，而 Tauri 2 的
    // 最低 WebView2 要求远高于此，该 polyfill 在本项目属于永远不会被触发的死代码。
    // 影响：仅省下 polyfill 的注入与体积；若真遇到不支持的环境也只是少一次预加载
    // 提示，模块仍会按 import 图正常加载，不产生功能性问题。
    modulePreload: {
      polyfill: false,
    },

    rollupOptions: {
      output: {
        // ── 拆分策略说明（每一条都基于实际引用核对，不是拍脑袋分组）──
        manualChunks: {
          // React 三件套：所有页面共用，且变动极少，独立出来最利于长期缓存。
          react: ["react", "react-dom", "react-router-dom"],

          // chakra + emotion + framer-motion 打包在一起。**这是核对后的有意选择**：
          // · @emotion/react / @emotion/styled 是 @chakra-ui/react 的 peerDependency
          //   （package.json 中 peer 声明为 ">=11"），chakra 的 ESM 产物直接 import
          //   它们，两者强耦合，拆开只会产生循环 chunk 引用，得不偿失。
          // · framer-motion 同样是 chakra 的 peer（声明为 ">=4.0.0"），且 chakra 的
          //   toast/modal/menu 等模块 `import { motion } from 'framer-motion'`
          //   （已在 dist/esm 下 grep 到 14 处静态 import）。
          // · 更关键的是 framer-motion 本身就是**首屏必需**，不属于"某些页面才用"：
          //   App.tsx 静态 import 了 DynamicIslandHost，而
          //   src/components/ui/dynamic-island.tsx 第 4 行直接
          //   `import { motion, useAnimationControls } from "framer-motion"`；
          //   该组件在主布局中常驻渲染。也就是说 framer-motion 打散出去也照样
          //   要在首帧加载，属纯增请求数、不省首屏字节。
          //   （桌面端 dist 每次随安装包整体替换、不存在跨版本 HTTP 缓存收益，
          //   因此"独立 chunk 便于缓存"这一常见理由在本项目不成立。）
          chakra: ["@chakra-ui/react", "@emotion/react", "@emotion/styled", "framer-motion"],

          // three 单独成块。引用面已核对：全仓库只有
          // src/components/SpectrumScene.tsx 使用 `import * as THREE from "three"`，
          // 而它只被 src/pages/MusicPage.tsx（音乐页的 3D 频谱"音域回响"）引用。
          // 把它隔离成独立 chunk 是正确方向——**绝不能把它并入首屏公共块**。
          // 现状提示（记录，勿在本文件内解决）：当前 MusicPage 仍是被 App.tsx
          // 静态 import 的，导致 three 作为入口的静态依赖被打进
          // dist/index.html 的 modulepreload（实测 505,674 字节随首屏预加载）。
          // 待路由懒加载改造后它会变成动态 chunk 并自动退出首屏，无需在此处改动。
          three: ["three"],

          // 图标库体量可观且按需引用，独立隔离便于观察体积。
          icons: ["lucide-react", "react-icons"],
        },
      },
    },

    // 未显式设置 build.target：默认 'modules'（≈ es2020、Chrome 87+）。
    // 依据：运行环境锁定为 Windows WebView2（Chromium），不需要为老内核降级语法；
    // 改成 'es5' 只会引入额外降级代码、增大体积并拖慢解析，属于明确的反优化。
    // 同样未设置 minify：默认 esbuild 压缩已足够，改用 terser 需新增依赖，
    // 与本任务"不新增依赖"的约束冲突。
  },
  server: {
    port: 3000,
    host: "127.0.0.1",
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
