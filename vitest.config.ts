/**
 * # vitest.config.ts — 前端单元测试与组件测试 Vitest 配置
 *
 * 为 Vitest 注册 Vue 单文件组件编译插件（@vitejs/plugin-vue）与 `@` 路径别名，
 * 使纯逻辑单测默认运行在轻量 Node 环境，组件测试通过顶部 `@vitest-environment happy-dom` 按需启用模拟 DOM。
 *
 * ## Key Exports
 * - 默认导出：Vitest 配置对象
 *
 * ## Dependencies
 * - External: `vitest/config`, `@vitejs/plugin-vue`
 */
import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";
import { fileURLToPath } from "node:url";

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  test: {
    // 默认保持 node 环境，保证既有 63 个纯逻辑/Store 单测极速执行；
    // .vue 组件测试文件在顶部声明 `// @vitest-environment happy-dom` 即可按需启用虚拟 DOM。
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
