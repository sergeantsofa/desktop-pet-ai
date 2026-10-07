/**
 * 整點報時 (Hourly Strike)
 * 
 * 每到整點(與上次報時間隔 >= 55 分鐘)就觸發 callback。
 * 支援自訂閒置期間(例如使用者睡覺時不報)。
 *
 * 用法:
 *   const stop = startStrike((hour) => say(`噹噹~ ${hour} 點囉!`));
 *   之後可呼叫 stop() 停止。
 */
export interface StrikeOptions {
  /**
   * 最短報時間隔(分),預設 55。
   * 如果設 60 且使用者一直醒著,每小時整點報一次;
   * 設 55 則容許一些時鐘誤差,不會漏掉。
   */
  minIntervalMinutes?: number;
  /** 不報時的凌晨時段開始(0~23),預設 0 (午夜) */
  quietStart?: number;
  /** 不報時的凌晨時段結束(0~23),預設 7 (早上) */
  quietEnd?: number;
}

const DEFAULTS: Required<StrikeOptions> = {
  minIntervalMinutes: 55,
  quietStart: 0,
  quietEnd: 7,
};

export type StrikeCallback = (hour: number) => void;

export function startStrike(
  callback: StrikeCallback,
  opts: StrikeOptions = {},
): () => void {
  const { minIntervalMinutes, quietStart, quietEnd } = { ...DEFAULTS, ...opts };

  let lastStrike = 0; // timestamp ms
  let timer: ReturnType<typeof setInterval> | null = null;

  function check(): void {
    const now = Date.now();
    const d = new Date(now);
    const hour = d.getHours();

    // 靜音時段不報
    if (quietStart <= quietEnd) {
      if (hour >= quietStart && hour < quietEnd) return;
    } else {
      // 跨日,例如 quietStart=22, quietEnd=6 → 22~23 & 0~5 靜音
      if (hour >= quietStart || hour < quietEnd) return;
    }

    // 距離上次報時夠久嗎?
    if (now - lastStrike < minIntervalMinutes * 60_000) return;

    // 檢查是否接近整點(前後 1 分鐘內)
    const minutes = d.getMinutes();
    const seconds = d.getSeconds();
    const totalSeconds = minutes * 60 + seconds;
    if (totalSeconds > 120 && totalSeconds < 3540) return; // 不在整點附近

    lastStrike = now;
    callback(hour);
  }

  // 每 30 秒檢查一次;週期短,開機後很快就能抓到第一個整點
  timer = setInterval(check, 30_000);

  // 啟動後立即檢查一次(也許正好是整點)
  check();

  return () => {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  };
}
