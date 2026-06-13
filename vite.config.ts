import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [vue()],

  // Tauri 開發時由 CLI 接管,避免清空終端輸出
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      // 避免監看 Rust 端目錄
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    // WebView2 (Windows) 對應的 Chromium 版本
    target: "chrome105",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
