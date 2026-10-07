import { createApp } from "vue";

// 依視窗 label 決定掛載哪個畫面:
//  - "settings" → 獨立的設定視窗(分頁式設定 UI)
//  - 其他(main / 開發瀏覽器)→ 桌寵本體
// 立刻掛載,不被任何資源載入卡住(Cubism Core 改由 App.vue 啟動時背景載入)。
async function mount(): Promise<void> {
  let isSettingsWindow = false;
  if ("__TAURI_INTERNALS__" in window) {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      isSettingsWindow = getCurrentWindow().label === "settings";
    } catch {
      /* 拿不到視窗資訊就當作主視窗 */
    }
  }

  if (isSettingsWindow) {
    const { default: SettingsPanel } = await import("./settings/SettingsPanel.vue");
    createApp(SettingsPanel).mount("#app");
  } else {
    const { default: App } = await import("./App.vue");
    createApp(App).mount("#app");
  }
}

void mount();
