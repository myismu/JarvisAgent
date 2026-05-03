import { createApp } from "vue";
import { createPinia } from "pinia";
import MonitorApp from "./MonitorApp.vue";
import "./assets/global.css"; // 全局样式（CSS 变量、重置、暗色模式）

const app = createApp(MonitorApp);
app.use(createPinia());
app.mount("#app");
