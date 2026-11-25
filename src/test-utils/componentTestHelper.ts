/**
 * # componentTestHelper.ts — Vue 组件测试公共初始化工具
 *
 * 为 `.vue` 组件测试统一创建隔离的 Pinia 状态实例与中文 `vue-i18n` 实例，
 * 避免每个测试文件重复编写样板挂载配置，确保各测试用例之间状态完全隔离。
 *
 * ## Key Exports
 * - `createTestPlugins()`: 生成隔离的 `[pinia, i18n]` 插件组合，供 `mount(..., { global: { plugins } })` 使用
 *
 * ## Dependencies
 * - Internal: `../locales/zh-CN.json`, `../locales/en-US.json`
 * - External: `pinia`, `vue-i18n`
 */
import { createPinia, setActivePinia } from "pinia";
import { createI18n } from "vue-i18n";
import zhCN from "../locales/zh-CN.json";
import enUS from "../locales/en-US.json";

/**
 * 为单个测试用例创建全新的 Pinia（状态仓库）与 i18n（国际化）实例。
 * 类似 Java JUnit 里每个 `@Test` 方法独立初始化测试上下文，防止上一个用例的残留状态污染下一个用例。
 */
export function createTestPlugins() {
  const pinia = createPinia();
  setActivePinia(pinia);

  const i18n = createI18n({
    legacy: false,
    locale: "zh-CN",
    fallbackLocale: "zh-CN",
    messages: {
      "zh-CN": zhCN,
      "en-US": enUS,
    },
  });

  return {
    pinia,
    i18n,
    plugins: [pinia, i18n],
  };
}
