/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<{}, {}, any>;
  export default component;
}

interface Window {
  /** Cubism Core,由 index.html 的 script 載入(使用者自行放置) */
  Live2DCubismCore?: unknown;
  /** Tauri runtime 注入,存在即代表在 Tauri 視窗內執行 */
  __TAURI_INTERNALS__?: unknown;
}
