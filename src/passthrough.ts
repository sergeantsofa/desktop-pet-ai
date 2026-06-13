/**
 * 智慧點擊穿透:游標不在角色(或任何 UI)上時,讓滑鼠事件穿透到下層視窗。
 * 視窗在穿透狀態收不到滑鼠事件,所以改由輪詢全域游標位置 + 前端命中測試來切換。
 * 由托盤「智慧穿透」開關(Rust 發 smart-passthrough 事件)啟用。
 */
import { isTauri } from "./llm/api";

export interface PassthroughOptions {
  /** 視窗內座標是否命中角色 */
  hits: (x: number, y: number) => boolean;
  /** 對話框/設定/泡泡等 UI 顯示中 → 整窗保持可互動 */
  uiActive: () => boolean;
}

const POLL_MS = 120;

let timer: number | undefined;
let paused = false;
let lastIgnore: boolean | null = null;
let options: PassthroughOptions | null = null;

export function startSmartPassthrough(opts: PassthroughOptions): void {
  if (!isTauri) return;
  options = opts;
  if (timer) return;
  timer = window.setInterval(() => void tick(), POLL_MS);
}

export async function stopSmartPassthrough(): Promise<void> {
  if (timer) {
    window.clearInterval(timer);
    timer = undefined;
  }
  lastIgnore = null;
  if (!isTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setIgnoreCursorEvents(false).catch(() => undefined);
}

/** 整窗手動穿透(托盤)優先時,暫停智慧輪詢以免互搶。 */
export function pauseSmartPassthrough(pause: boolean): void {
  paused = pause;
  lastIgnore = null; // 恢復後重新校準
}

let ticking = false;

async function tick(): Promise<void> {
  if (paused || !options || ticking) return;
  ticking = true;
  try {
    const { getCurrentWindow, cursorPosition } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    const [cursor, winPos, scale] = await Promise.all([
      cursorPosition(),
      win.outerPosition(),
      win.scaleFactor(),
    ]);
    const x = (cursor.x - winPos.x) / scale;
    const y = (cursor.y - winPos.y) / scale;
    const inside = x >= 0 && y >= 0 && x <= window.innerWidth && y <= window.innerHeight;
    const interactive = inside && (options.uiActive() || options.hits(x, y));
    const ignore = !interactive;
    if (ignore !== lastIgnore) {
      lastIgnore = ignore;
      await win.setIgnoreCursorEvents(ignore);
    }
  } catch {
    /* 視窗關閉中等暫態錯誤,下一輪再試 */
  } finally {
    ticking = false;
  }
}
