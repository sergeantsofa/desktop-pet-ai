import { createApp } from "vue";
import App from "./App.vue";
import { ensureCubismCore } from "./resources";

// 打包版的 Cubism Core 可能在外部 appdata\vendor\,啟動時先確保載入再掛載 App
void ensureCubismCore().finally(() => {
  createApp(App).mount("#app");
});
