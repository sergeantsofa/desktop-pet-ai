/**
 * 外觀設定:角色大小(存 localStorage)、視窗尺寸(存後端 window.rs;這裡只給
 * 即時讀取/設定的 Tauri helper)。純前端,非 Tauri 環境下都優雅降級。
 */
import { isTauri } from "./llm/api";

/* ---------------- 角色大小(Live2D userScale) ---------------- */

const CHAR_SCALE_KEY = "pet-char-scale";
export const CHAR_SCALE_MIN = 0.5;
export const CHAR_SCALE_MAX = 2.0;
export const DEFAULT_CHAR_SCALE = 1.0;

function clampScale(n: number): number {
  return Math.min(CHAR_SCALE_MAX, Math.max(CHAR_SCALE_MIN, n));
}

/** 讀取使用者的角色縮放倍率(壞值/沒設過 → 預設 1)。 */
export function loadCharScale(): number {
  const raw = localStorage.getItem(CHAR_SCALE_KEY);
  const n = raw ? Number(raw) : NaN;
  return Number.isFinite(n) ? clampScale(n) : DEFAULT_CHAR_SCALE;
}

/** 儲存角色縮放倍率。 */
export function saveCharScale(scale: number): void {
  localStorage.setItem(CHAR_SCALE_KEY, String(clampScale(scale)));
}

/* ---------------- 對話泡泡樣式 ---------------- */

export interface BubbleStyleOption {
  id: string;
  label: string;
}

/** 可選的對話泡泡樣式(id 對應 SpeechBubble 的 `bubble--<id>` class) */
export const BUBBLE_STYLES: BubbleStyleOption[] = [
  { id: "classic", label: "經典白" },
  { id: "cloud", label: "雲朵(圓潤)" },
  { id: "comic", label: "漫畫框(粗黑邊)" },
  { id: "neon", label: "霓虹發光" },
  { id: "pastel", label: "粉彩漸層" },
  { id: "pixel", label: "像素風" },
  { id: "candy", label: "糖果(暖色)" },
  { id: "ghost", label: "透明玻璃" },
];

const BUBBLE_STYLE_KEY = "pet-bubble-style";
export const DEFAULT_BUBBLE_STYLE = "classic";

/** 讀取目前選的泡泡樣式(沒設過/不認得 → 預設經典白)。 */
export function loadBubbleStyle(): string {
  const v = localStorage.getItem(BUBBLE_STYLE_KEY);
  return v && BUBBLE_STYLES.some((s) => s.id === v) ? v : DEFAULT_BUBBLE_STYLE;
}

/** 儲存泡泡樣式。 */
export function saveBubbleStyle(id: string): void {
  localStorage.setItem(BUBBLE_STYLE_KEY, id);
}

/* ---------------- 視窗尺寸(邏輯像素) ---------------- */

export const WIN_MIN = { w: 220, h: 300 };
export const WIN_MAX = { w: 3840, h: 2160 };

/**
 * 取得「角色視窗(main)」的控制把手。即使這段程式跑在設定視窗,也永遠指向 main,
 * 確保「視窗大小」調的是桌寵本體、不是設定視窗本身。
 */
async function mainWindow() {
  if (!isTauri) return null;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const cur = getCurrentWindow();
  if (cur.label === "main") return cur;
  const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  return (await WebviewWindow.getByLabel("main")) ?? null;
}

/** 取角色視窗(main)的邏輯尺寸(寬高);非 Tauri / 取不到回 null。 */
export async function getWindowSize(): Promise<{ w: number; h: number } | null> {
  const win = await mainWindow();
  if (!win) return null;
  try {
    const [size, sf] = await Promise.all([win.innerSize(), win.scaleFactor()]);
    return { w: Math.round(size.width / sf), h: Math.round(size.height / sf) };
  } catch {
    return null;
  }
}

/** 設定角色視窗(main)的邏輯尺寸(會夾在上下限內);後端 window.rs 會在 Resized 時自動存檔。 */
export async function setWindowSize(w: number, h: number): Promise<void> {
  const win = await mainWindow();
  if (!win) return;
  const cw = Math.min(WIN_MAX.w, Math.max(WIN_MIN.w, Math.round(w)));
  const ch = Math.min(WIN_MAX.h, Math.max(WIN_MIN.h, Math.round(h)));
  const { LogicalSize } = await import("@tauri-apps/api/dpi");
  await win.setSize(new LogicalSize(cw, ch));
}
