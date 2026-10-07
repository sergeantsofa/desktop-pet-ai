/**
 * 任務自動分流:依使用者訊息內容,把對話路由到 chat / coder / reasoner。
 * 刻意保守——只有明確的程式/數理訊號才分流,其餘一律走 chat(免得閒聊被誤判)。
 * 對應 provider.rs 的 routing[task];實際模型在「設定 → 任務路由」各自指定。
 */
export type RouteTask = "chat" | "coder" | "reasoner";

/** 程式相關訊號(命中任一 → coder) */
const CODER_PATTERNS: RegExp[] = [
  /```|~~~/, // 程式碼區塊
  /\b(function|const|let|var|class|import|export|def|async|await|return|public|private|void|null|undefined|console\.log|print\()\b/i,
  /\b(npm|pnpm|yarn|pip|cargo|git|docker|sql|regex|stdout|stderr|localhost|http[s]?:\/\/)\b/i,
  /\b(python|javascript|typescript|java|kotlin|swift|rust|golang|php|ruby|c\+\+|c#|vue|react|node|nginx|linux)\b/i,
  /\.(py|js|ts|tsx|jsx|rs|java|cpp|cs|go|rb|php|html|css|json|vue|sh|sql)\b/i,
  /(寫|幫我寫|幫我改|幫我修|幫我debug|優化|重構).{0,8}(程式|程式碼|函式|腳本|爬蟲|網頁|app|功能)/,
  /(程式碼|函式|變數|陣列|迴圈|字串處理|物件導向|資料結構|演算法|編譯錯誤|語法錯誤|執行時錯誤|報錯|錯誤訊息|例外|stack ?trace)/i,
  /(bug|debug|compile|exception|traceback)/i,
];

/** 數理/邏輯推理訊號(保守,避免把閒聊的「為什麼」也算進去 → reasoner) */
const REASONER_PATTERNS: RegExp[] = [
  /\d+\s*[+*^×÷]\s*\d+/, // 明確算式(避開 - 和 / 以免誤判日期/範圍)
  /(計算|算一下|幫我算|求.{0,4}(值|解|答案|面積|體積|周長)|解方程|證明一下|推導|機率是多少|排列組合|微分|積分|因式分解|質因數|最大公因數|最小公倍數|數學題|邏輯推理|算術)/,
];

/** 依訊息內容挑任務;沒有明確訊號就回 chat。 */
export function pickTask(text: string): RouteTask {
  const t = (text || "").trim();
  if (!t) return "chat";
  if (CODER_PATTERNS.some((re) => re.test(t))) return "coder";
  if (REASONER_PATTERNS.some((re) => re.test(t))) return "reasoner";
  return "chat";
}
