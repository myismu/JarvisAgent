import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./assets/global.css"; // 全局样式（CSS 变量、重置、暗色模式）

const app = createApp(App);
app.use(createPinia());
app.mount("#app");
