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
    chunkSizeWarningLimit: 1800,
    rollupOptions: {
      output: {
        manualChunks: {
          react: ["react", "react-dom", "react-router-dom"],
          chakra: ["@chakra-ui/react", "@emotion/react", "@emotion/styled", "framer-motion"],
          three: ["three"],
          icons: ["lucide-react", "react-icons"],
        },
      },
    },
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
