/**
 * 自動更新(Tauri updater)。
 *
 * 兩種使用情境:
 *  1. 啟動時靜默檢查(updateTick)→ 有新版就整個自動跑完(下載→安裝→重啟),
 *     「已最新」或「連不上」都不吵使用者。
 *  2. 設定面板的「關於 / 更新」分頁 → 手動檢查,每一種結果都要有明確回饋
 *     (包含失敗原因,絕不靜默吞掉)。
 *
 * 設定存 localStorage:
 *  - pet-update-settings:{ autoCheck, autoDownload, backupBeforeUpdate, proxy }
 *  - pet-update-last-check:上次自動檢查的時間戳(24 小時節流用)
 */
import { isTauri } from "./llm/api";

/* ---------------- 設定 ---------------- */

export interface UpdateSettings {
  /** 啟動時自動檢查更新(預設開) */
  autoCheck: boolean;
  /** 檢查到新版就自動下載+安裝+重啟(預設開) */
  autoDownload: boolean;
  /** 更新前自動備份資料(記憶/設定/模型;預設開) */
  backupBeforeUpdate: boolean;
  /** 更新用的 HTTP 代理(連不上 GitHub 的環境填,例如 http://127.0.0.1:7890) */
  proxy: string;
}

const SETTINGS_KEY = "pet-update-settings";
const LAST_CHECK_KEY = "pet-update-last-check";

/** 自動檢查的節流間隔(24 小時) */
const AUTO_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
/** 檢查更新的逾時(毫秒) */
const CHECK_TIMEOUT_MS = 15_000;
/** 下載失敗自動重試次數 */
const DOWNLOAD_RETRIES = 1;

export function loadUpdateSettings(): UpdateSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (raw) {
      const p = JSON.parse(raw) as Partial<UpdateSettings>;
      return {
        autoCheck: p.autoCheck !== false,
        autoDownload: p.autoDownload !== false,
        backupBeforeUpdate: p.backupBeforeUpdate !== false,
        proxy: typeof p.proxy === "string" ? p.proxy : "",
      };
    }
  } catch {
    /* 壞掉就用預設 */
  }
  return { autoCheck: true, autoDownload: true, backupBeforeUpdate: true, proxy: "" };
}

export function saveUpdateSettings(s: UpdateSettings): void {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
  } catch {
    /* 忽略 */
  }
}

function lastCheckAt(): number {
  const raw = localStorage.getItem(LAST_CHECK_KEY);
  const n = raw ? Number(raw) : 0;
  return Number.isFinite(n) ? n : 0;
}

function markChecked(): void {
  try {
    localStorage.setItem(LAST_CHECK_KEY, String(Date.now()));
  } catch {
    /* 忽略 */
  }
}

/** 上次檢查時間(毫秒),0 = 從未檢查 */
export function getLastCheckAt(): number {
  return lastCheckAt();
}

/* ---------------- 檢查選項 ---------------- */

interface CheckOptions {
  headers?: Record<string, string>;
  timeout?: number;
  proxy?: string;
}

/** 依設定組出 check() 的選項;代理留空就不要帶 proxy 欄位 */
function checkOptions(): CheckOptions {
  const { proxy } = loadUpdateSettings();
  const opts: CheckOptions = { timeout: CHECK_TIMEOUT_MS };
  if (proxy.trim()) opts.proxy = proxy.trim();
  return opts;
}

/* ---------------- 錯誤訊息(人看得懂) ---------------- */

/**
 * 把外掛丟出的原始錯誤轉成可行動的中文說明。
 * 更新最怕「靜默失敗」,所以這裡刻意區分網路 / 簽章 / 其他三類。
 */
export function describeUpdateError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err);
  const s = raw.toLowerCase();

  if (s.includes("signature") || s.includes("minisign") || s.includes("public key")) {
    return `簽章驗證失敗(更新檔與內嵌公鑰不符,可能是發布時用錯了金鑰):${raw}`;
  }
  if (
    s.includes("dns") ||
    s.includes("connect") ||
    s.includes("timed out") ||
    s.includes("timeout") ||
    s.includes("network") ||
    s.includes("tls") ||
    s.includes("certificate") ||
    s.includes("proxy")
  ) {
    return `連不上更新伺服器(GitHub)。請確認網路,或到下方「進階」設定更新代理。原因:${raw}`;
  }
  if (s.includes("404") || s.includes("not found")) {
    return `更新資訊不存在(這個版本可能還沒發布 release)。原因:${raw}`;
  }
  return `更新失敗:${raw}`;
}

/* ---------------- 輕量:啟動時靜默檢查 ---------------- */

export interface UpdateInfo {
  version: string;
  notes: string;
  /** 下載並安裝,完成後重啟 App */
  install: () => Promise<void>;
}

/**
 * 檢查更新;有新版回傳 UpdateInfo,否則 null。
 * 任何錯誤(離線、簽章不符等)都吞掉回 null —— 這是給「啟動時靜默檢查」用的,
 * 不要拿它做需要明確回饋的 UI(那請用 checkForUpdateDetailed)。
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  if (!isTauri) return null;
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    let update;
    try {
      update = await check(checkOptions());
    } catch (e) {
      // 舊版外掛不認得 timeout/proxy 參數時,退回不帶選項再試一次
      if (!checkOptions().proxy) throw e;
      update = await check();
    }
    if (!update) return null;
    return {
      version: update.version,
      notes: update.body ?? "",
      install: async () => {
        await update.downloadAndInstall(undefined, {
          timeout: 10 * 60 * 1000,
        });
        const { relaunch } = await import("@tauri-apps/plugin-process");
        await relaunch();
      },
    };
  } catch {
    return null; // 靜默:當作沒更新
  }
}

/**
 * 啟動時的自動更新排程:
 *  - 未開啟自動檢查 → 什麼都不做
 *  - 距上次檢查未滿 24 小時 → 跳過
 *  - 有新版且開了自動下載 → 直接跑完整流程(下載→安裝→重啟)
 *
 * @param onStatus 給 UI 顯示目前階段(可選)
 * @returns 是否有採取行動
 */
export async function updateTick(
  onStatus?: (text: string) => void
): Promise<"skipped" | "up-to-date" | "auto-updating" | "notified" | "failed"> {
  if (!isTauri) return "skipped";
  const cfg = loadUpdateSettings();
  if (!cfg.autoCheck) return "skipped";
  if (Date.now() - lastCheckAt() < AUTO_CHECK_INTERVAL_MS) return "skipped";

  onStatus?.("檢查更新中…");
  const info = await checkForUpdate(); // 靜默版:失敗回 null
  markChecked();
  if (!info) return "failed";

  if (!cfg.autoDownload) return "notified";

  // 全自動:備份 → 下載 → 安裝 → 重啟
  try {
    if (cfg.backupBeforeUpdate) {
      onStatus?.("更新前先備份資料…");
      await backupAppData().catch(() => undefined); // 備份失敗不擋更新
    }
    onStatus?.(`發現新版本 v${info.version},自動更新中…`);
    await info.install();
    return "auto-updating";
  } catch {
    return "failed";
  }
}

/* ---------------- 完整:設定面板用 ---------------- */

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date"; current: string }
  | {
      kind: "available";
      current: string;
      version: string;
      date?: string;
      notes: string;
    }
  | { kind: "downloading"; downloaded: number; total?: number }
  | { kind: "backing-up" }
  | { kind: "installing" }
  | { kind: "done" }
  | { kind: "error"; message: string };

/** 目前 App 版本(供 UI 顯示) */
export async function currentVersion(): Promise<string> {
  if (!isTauri) return "dev";
  try {
    const { getVersion } = await import("@tauri-apps/api/app");
    return await getVersion();
  } catch {
    return "未知";
  }
}

type UpdateHandle = Awaited<
  ReturnType<typeof import("@tauri-apps/plugin-updater").check>
>;

/**
 * 手動檢查更新。回傳完整的狀態轉移序列,呼叫端用 onState 更新 UI。
 * 與 checkForUpdate 不同:**錯誤一律往上報**,不吞掉。
 */
export async function checkForUpdateDetailed(
  onState: (s: UpdateState) => void
): Promise<UpdateHandle> {
  onState({ kind: "checking" });

  if (!isTauri) {
    onState({ kind: "error", message: "開發模式(非 Tauri 環境)無法檢查更新。" });
    return null;
  }

  const { check } = await import("@tauri-apps/plugin-updater");
  const current = await currentVersion();

  let update: UpdateHandle;
  try {
    update = await check(checkOptions());
  } catch (e) {
    // 同上:不認得選項就退回無選項再試
    try {
      update = await check();
    } catch {
      onState({ kind: "error", message: describeUpdateError(e) });
      return null;
    }
  }

  markChecked();
  if (!update) {
    onState({ kind: "up-to-date", current });
    return null;
  }

  onState({
    kind: "available",
    current,
    version: update.version,
    date: update.date,
    notes: update.body ?? "",
  });
  return update;
}

/** 下載並安裝(含進度回報 + 失敗自動重試一次),完成後重啟 App。 */
export async function downloadAndInstall(
  update: NonNullable<UpdateHandle>,
  onState: (s: UpdateState) => void
): Promise<void> {
  const run = async (): Promise<void> => {
    let downloaded = 0;
    let total: number | undefined;
    await update.downloadAndInstall((e) => {
      if (e.event === "Started") {
        downloaded = 0;
        total = e.data.contentLength;
        onState({ kind: "downloading", downloaded, total });
      } else if (e.event === "Progress") {
        downloaded += e.data.chunkLength;
        onState({ kind: "downloading", downloaded, total });
      } else if (e.event === "Finished") {
        onState({ kind: "installing" });
      }
    });
  };

  let lastErr: unknown;
  for (let attempt = 0; attempt <= DOWNLOAD_RETRIES; attempt++) {
    try {
      await run();
      onState({ kind: "done" });
      const { relaunch } = await import("@tauri-apps/plugin-process");
      await relaunch();
      return;
    } catch (e) {
      lastErr = e;
      if (attempt < DOWNLOAD_RETRIES) {
        onState({
          kind: "downloading",
          downloaded: 0,
          total: undefined,
        });
      }
    }
  }
  onState({ kind: "error", message: describeUpdateError(lastErr) });
}

/** 開 GitHub Releases 頁(「查看所有版本」用) */
export async function openReleasesPage(): Promise<void> {
  const url = "https://github.com/sergeantsofa/desktop-pet-ai/releases";
  if (isTauri) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("open_external_url", { url });
      return;
    } catch {
      /* 命令不可用時退回瀏覽器開 */
    }
  }
  window.open(url, "_blank");
}

/** 把位元組數格式化成人看的字串 */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/* ---------------- 更新前備份(保險) ---------------- */

export interface BackupResult {
  path: string;
  files: number;
  bytes: number;
  skipped: number;
  kept: number;
  label: string;
  memory_included: boolean;
}

export interface BackupInfo {
  label: string;
  path: string;
  bytes: number;
}

/** 立即把 App 資料(記憶/設定/模型)快照一份 */
export async function backupAppData(): Promise<BackupResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<BackupResult>("backup_appdata");
}

/** 列出目前保留的備份(新到舊) */
export async function listBackups(): Promise<BackupInfo[]> {
  if (!isTauri) return [];
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<BackupInfo[]>("list_backups");
  } catch {
    return [];
  }
}

/** 用系統檔案總管打開資料夾(備份在底下的 backups\);還原時把內容覆蓋回去 */
export async function openBackupsFolder(): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("open_data_folder");
}
