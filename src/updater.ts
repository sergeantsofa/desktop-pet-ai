/**
 * 自動更新(Tauri updater)。啟動時靜默檢查 GitHub Releases,
 * 有新版就回報給 UI,由桌寵主動問使用者要不要更新。
 */
import { isTauri } from "./llm/api";

export interface UpdateInfo {
  version: string;
  notes: string;
  /** 下載並安裝,完成後重啟 App */
  install: () => Promise<void>;
}

/** 檢查更新;有新版回傳 UpdateInfo,否則 null。任何錯誤(離線等)都吞掉回 null。 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  if (!isTauri) return null;
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (!update) return null;
    return {
      version: update.version,
      notes: update.body ?? "",
      install: async () => {
        await update.downloadAndInstall();
        const { relaunch } = await import("@tauri-apps/plugin-process");
        await relaunch();
      },
    };
  } catch {
    return null; // 離線、無 release、簽章不符等 → 當作沒更新
  }
}
