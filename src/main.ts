import { createApp } from "vue";
import App from "./App.vue";

// 立刻掛載 UI,不被任何資源載入卡住(Cubism Core 改由 App.vue 啟動時背景載入)
createApp(App).mount("#app");
