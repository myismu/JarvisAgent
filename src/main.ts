import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { i18n } from "./i18n";
import "./assets/global.css"; // 全局样式（CSS 变量、重置、暗色模式）

const app = createApp(App);
app.use(createPinia());
app.use(i18n);
app.mount("#app");
