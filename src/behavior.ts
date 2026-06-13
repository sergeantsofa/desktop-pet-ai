/**
 * 主動行為設定(M4.5,純前端,存 localStorage)。
 * 提醒(reminder)永遠開啟;這裡只控制「閒置主動找話題」,因為它會消耗 API。
 */

export interface BehaviorSettings {
  /** 閒置太久時主動找話題 */
  proactiveChat: boolean;
  /** 閒置幾分鐘後主動(1~120) */
  idleMinutes: number;
}

const STORAGE_KEY = "behavior-settings";

const DEFAULTS: BehaviorSettings = {
  proactiveChat: true,
  idleMinutes: 15,
};

export function loadBehavior(): BehaviorSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    const s = { ...DEFAULTS, ...(JSON.parse(raw) as Partial<BehaviorSettings>) };
    s.idleMinutes = Math.min(120, Math.max(1, s.idleMinutes));
    return s;
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveBehavior(settings: BehaviorSettings): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
}
