import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
// @ts-expect-error node:url 是 nodejs 内置模块，本仓库未装 @types/node
import { fileURLToPath } from "node:url";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
// @ts-expect-error process is a nodejs global
const isWindowsTarget = process.env.TAURI_ENV_PLATFORM === "windows";
// @ts-expect-error process is a nodejs global
const isDebugBuild = !!process.env.TAURI_ENV_DEBUG;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [vue()],

  // 路径别名：`@` → `src`。
  // 全仓目前仍是 `../../` 相对路径，迁移是渐进的，这里先把口子留好：新文件直接用 `@/...`，
  // 旧文件改到哪算哪——避免大组件挪位置时批量断链。
  // 必须与 tsconfig.json 的 compilerOptions.paths 保持同一口径，否则编辑器与构建会各说各话。
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri` and `memory.jsonl`
      ignored: ["**/src-tauri/**", "**/memory.jsonl"],
    },
  },
  build: {
    // 4. 目标是各平台 WebView 的内核基线（官方模板口径）：Windows 是 WebView2(Chromium)，
    //    其余是 WKWebView。不显式指定时 Vite 按最新 ESNext 产出，老内核上可能直接白屏。
    target: isWindowsTarget ? "chrome105" : "safari13",
    // 5. 调试构建保留 sourcemap 且不压缩，方便在 WebView 里定位到原始代码
    minify: isDebugBuild ? false : "esbuild",
    sourcemap: isDebugBuild,
    rollupOptions: {
      input: {
        main: "index.html",
        monitor: "monitor.html",
      },
      output: {
        // 6. 把体积大、很少变动的三方库拆成独立 chunk。
        //    桌面端从本地磁盘加载，收益主要在「改业务代码时这几块不必重新解析」。
        manualChunks(id) {
          if (!id.includes("node_modules")) return;
          if (id.includes("marked")) return "vendor-markdown";
          if (id.includes("vue-i18n") || id.includes("@intlify")) return "vendor-i18n";
          if (id.includes("vue") || id.includes("pinia")) return "vendor-vue";
          return "vendor";
        },
      },
    },
  },
}));
