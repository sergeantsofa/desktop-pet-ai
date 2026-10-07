//! 更新前的保險:把「她的全部資料」快照一份。
//!
//! 涵蓋 `%APPDATA%\com.desktoppet.ai\` 底下的:
//! - `memory.db`    長期記憶 + 對話紀錄 + 網頁帳號 + 提醒(gamekb / persona 都在這)
//! - `settings.json` 所有設定(人設、路由、連動、遠端綁定…)
//! - `models/`      角色清單與模型本體
//! - `vendor/`      Cubism Core
//!
//! 備份放 `backups/<時間>/`,只保留最近 [`KEEP`] 份。
//! 更新失敗 / 新版有問題時,把整個資料夾覆蓋回去就能回到原狀。
//!
//! 這裡刻意**不碰備份資料夾自己**(避免遞迴),並跳過 symlink / reparse point
//! (否則可能無限展開或複製到雲端預留位置)。

use std::path::Path;
use serde::Serialize;
use tauri::{AppHandle, Manager};

/// 保留幾份備份(超過就砍最舊的)
const KEEP: usize = 3;

#[derive(Debug, Serialize)]
pub struct BackupResult {
    /// 備份資料夾的完整路徑
    pub path: String,
    /// 複製了幾個檔案
    pub files: u64,
    /// 總位元組
    pub bytes: u64,
    /// 被跳過的檔案數(單檔失敗不中斷整份備份)
    pub skipped: u64,
    /// 這次備份後,總共留了幾份
    pub kept: usize,
    /// 這份備份的資料夾名稱(時間戳)
    pub label: String,
    /// 有沒有備份到記憶資料庫(最重要的一份)
    pub memory_included: bool,
}

/// 立即把 App 資料夾快照一份。回傳備份資訊。
#[tauri::command]
pub fn backup_appdata(app: AppHandle) -> Result<BackupResult, String> {
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if !root.is_dir() {
        return Err(format!("找不到資料夾:{}", root.display()));
    }
    backup_dir(&root)
}

/// 列出目前所有的備份(新到舊),給 UI 顯示。
#[tauri::command]
pub fn list_backups(app: AppHandle) -> Result<Vec<BackupInfo>, String> {
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(enumerate(&root.join("backups")))
}

#[derive(Debug, Serialize)]
pub struct BackupInfo {
    pub label: String,
    pub path: String,
    pub bytes: u64,
}

fn backup_dir(root: &Path) -> Result<BackupResult, String> {
    let backups = root.join("backups");
    std::fs::create_dir_all(&backups).map_err(|e| format!("建立備份資料夾失敗:{e}"))?;

    let label = chrono::Local::now().format("%Y-%m-%d_%H%M%S").to_string();
    let dest = backups.join(&label);
    // 同一秒內按兩次:換一個不衝突的名字
    let dest = if dest.exists() {
        backups.join(format!("{label}_1"))
    } else {
        dest
    };
    std::fs::create_dir_all(&dest).map_err(|e| format!("建立備份失敗:{e}"))?;

    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut skipped = 0u64;
    copy_dir(root, &dest, &backups, &mut files, &mut bytes, &mut skipped)
        .map_err(|e| format!("備份失敗:{e}"))?;

    // 先算「備份完之後總共會有幾份」,再修剪(prune 內部只保留最新的 KEEP 份)
    let total_before = enumerate(&backups).len();
    let kept = total_before.min(KEEP);
    prune(&backups, KEEP);
    let memory_included = dest.join("memory.db").is_file();

    Ok(BackupResult {
        path: dest.display().to_string(),
        files,
        bytes,
        skipped,
        kept,
        label: dest
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(label),
        memory_included,
    })
}

/// 遞迴複製。`root` 是來源根、`dest` 是目標根、`skip` 是要跳過的子樹(備份夾本身)。
fn copy_dir(
    src: &Path,
    dest: &Path,
    skip: &Path,
    files: &mut u64,
    bytes: &mut u64,
    skipped: &mut u64,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => {
            *skipped += 1;
            return Ok(());
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();

        // 不把備份資料夾自己複進去(否則會遞迴長大)
        if path == *skip {
            continue;
        }
        // 跳過 symlink / reparse point(雲端預留位置、junction 都可能出問題)
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            *skipped += 1;
            continue;
        }

        let target = dest.join(&name);
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => {
                *skipped += 1;
                continue;
            }
        };

        if ft.is_dir() {
            match std::fs::create_dir_all(&target) {
                Ok(_) => copy_dir(&path, &target, skip, files, bytes, skipped)?,
                Err(_) => *skipped += 1,
            }
        } else if ft.is_file() {
            match std::fs::copy(&path, &target) {
                Ok(n) => {
                    *files += 1;
                    *bytes += n;
                }
                Err(_) => *skipped += 1, // 單檔失敗(被鎖住等)不中斷整份備份
            }
        }
    }
    Ok(())
}

/// 只留最新的 `keep` 份,回傳實際保留數。
fn prune(backups: &Path, keep: usize) -> usize {
    let dirs = enumerate(backups); // 已是「新到舊」
    let total = dirs.len();
    if total <= keep {
        return total; // 不到上限就什麼都不砍(注意:drain(keep..) 在 keep > len 時會 panic)
    }
    let mut kept = dirs;
    for old in kept.drain(keep..) {
        let _ = std::fs::remove_dir_all(&old.path);
    }
    kept.len()
}

/// 列出 `backups/` 底下所有備份,依名稱(時間戳)新到舊排序。
fn enumerate(backups: &Path) -> Vec<BackupInfo> {
    let Ok(entries) = std::fs::read_dir(backups) else {
        return Vec::new();
    };
    let mut out: Vec<BackupInfo> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| {
            let path = e.path();
            BackupInfo {
                label: e.file_name().to_string_lossy().to_string(),
                bytes: dir_size(&path),
                path: path.display().to_string(),
            }
        })
        .collect();
    // 名稱是 YYYY-MM-DD_HHMMSS,字串反向排序即時間新到舊
    out.sort_by(|a, b| b.label.cmp(&a.label));
    out
}

/// 算一個資料夾的總大小(僅供 UI 顯示,失敗就當 0)
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dpai-backup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn copies_files_and_skips_backups_dir() {
        let root = tmp("basic");
        std::fs::write(root.join("settings.json"), b"{}").unwrap();
        std::fs::write(root.join("memory.db"), b"dbdbdb").unwrap();
        std::fs::create_dir_all(root.join("models/icegirl")).unwrap();
        std::fs::write(root.join("models/icegirl/a.model3.json"), b"m").unwrap();

        let r = backup_dir(&root).unwrap();
        assert_eq!(r.files, 3);
        assert!(r.memory_included);
        assert!(Path::new(&r.path).join("memory.db").is_file());
        assert!(Path::new(&r.path).join("models/icegirl/a.model3.json").is_file());

        // 再備一次:不能把上一次的備份也複進去(會遞迴長大)
        let r2 = backup_dir(&root).unwrap();
        assert_eq!(r2.files, 3, "第二次備份應仍只有 3 個檔,不該把 backups 自己算進去");
        assert!(!Path::new(&r2.path).join("backups").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keeps_only_latest_three() {
        let root = tmp("prune");
        std::fs::write(root.join("settings.json"), b"{}").unwrap();
        for i in 0..5 {
            std::fs::create_dir_all(root.join(format!("backups/2026-01-0{i}_000000"))).unwrap();
        }
        let r = backup_dir(&root).unwrap();
        assert_eq!(r.kept, KEEP);
        let listed = enumerate(&root.join("backups"));
        assert_eq!(listed.len(), KEEP, "超過上限應只留最新 {KEEP} 份");
        // 最新的排最前面
        assert_eq!(listed[0].label, r.label);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_keeps_newest_by_name() {
        let root = tmp("order");
        let b = root.join("backups");
        for n in ["2026-01-01_000000", "2026-03-01_000000", "2026-02-01_000000"] {
            std::fs::create_dir_all(b.join(n)).unwrap();
        }
        assert_eq!(prune(&b, 2), 2);
        let left = enumerate(&b);
        assert_eq!(left[0].label, "2026-03-01_000000");
        assert_eq!(left[1].label, "2026-02-01_000000");

        let _ = std::fs::remove_dir_all(&root);
    }
}
