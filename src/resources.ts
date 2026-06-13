/**
 * 資源(模型 / Cubism Core)載入位置解析。
 *
 * 發行版使用者把模型放到外部可寫資料夾 %APPDATA%\com.desktoppet.ai\models|vendor\,
 * Rust 端用本機 http 伺服器(fileserver.rs)serve 它,前端用 http://127.0.0.1:<port>/ 載入。
 * 取不到 port(伺服器沒起來)時回 null,呼叫端退回打包的 /models、/vendor。
 */
import { isTauri } from "./llm/api";

let basePromise: Promise<string | null> | null = null;

/** 本機資源伺服器的 base URL(如 http://127.0.0.1:51234);非 Tauri 或沒起來回 null。 */
export function resourceBase(): Promise<string | null> {
  if (basePromise) return basePromise;
  basePromise = (async () => {
    if (!isTauri) return null;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const port = await invoke<number>("resource_port");
      return port > 0 ? `http://127.0.0.1:${port}` : null;
    } catch {
      return null;
    }
  })();
  return basePromise;
}

/** 確保 Cubism Core 已載入:已在(打包 index.html)就跳過,否則試外部 vendor。 */
export async function ensureCubismCore(): Promise<boolean> {
  if (window.Live2DCubismCore) return true;
  const base = await resourceBase();
  if (!base) return false;
  try {
    await loadScript(`${base}/vendor/live2dcubismcore.min.js`);
    return !!window.Live2DCubismCore;
  } catch {
    return false;
  }
}

function loadScript(src: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const el = document.createElement("script");
    // 逾時保險:某些環境(防火牆擋本機 server)連線不回應,避免永遠 pending
    const timer = window.setTimeout(() => reject(new Error("script 載入逾時")), 8000);
    el.src = src;
    el.onload = () => {
      window.clearTimeout(timer);
      resolve();
    };
    el.onerror = () => {
      window.clearTimeout(timer);
      reject(new Error("script 載入失敗"));
    };
    document.head.appendChild(el);
  });
}
