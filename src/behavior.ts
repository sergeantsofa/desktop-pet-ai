/**
 * 主動行為設定(M4.5,純前端,存 localStorage)。
 * 提醒(reminder)永遠開啟;這裡只控制「閒置主動找話題」與「整點報時」。
 */

export interface BehaviorSettings {
  /** 閒置太久時主動找話題 */
  proactiveChat: boolean;
  /** 閒置幾分鐘後主動(1~120) */
  idleMinutes: number;
  /** 每整點報時 */
  hourlyChime: boolean;
  /** 整點報時的聲音(預設輕柔) */
  chimeStyle: "gentle" | "playful" | "minimal";
  /** 連續對話模式:講完後靜音幾秒算斷句(Ctrl+Shift+D) */
  converseSilenceSec: number;
}

const STORAGE_KEY = "behavior-settings";

const DEFAULTS: BehaviorSettings = {
  proactiveChat: true,
  idleMinutes: 15,
  hourlyChime: true,
  chimeStyle: "gentle",
  converseSilenceSec: 2,
};

export function loadBehavior(): BehaviorSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    const s = { ...DEFAULTS, ...(JSON.parse(raw) as Partial<BehaviorSettings>) };
    s.idleMinutes = Math.min(120, Math.max(1, s.idleMinutes));
    s.converseSilenceSec = Math.min(10, Math.max(0.5, s.converseSilenceSec));
    return s;
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveBehavior(settings: BehaviorSettings): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
}

/**
 * 根據 chimeStyle 產生一個適合整點報時的提示句
 */
export function chimePrompt(hour: number, style: string): string {
  const ampm = hour < 12 ? "早上" : hour < 18 ? "下午" : "晚上";
  const h12 = hour % 12 || 12;
  const base = `現在${ampm}${h12}點整~`;

  switch (style) {
    case "playful":
      return `(系統:現在整點到了。請你用俏皮、可愛的口氣說「${base}」,可以加一句關心或閒聊,像「該起來動一動啦」或「喝口水吧」。直接說,不要提到這是系統訊息。)`;
    case "minimal":
      return `(系統:現在整點。請你簡短地說「${base}」就好。)`;
    default: // gentle
      return `(系統:現在整點。請你以溫暖、輕柔的語氣說「${base}」,可以加一點點小關心,不要超過一句話。直接說,不要提系統。)`;
  }
}
