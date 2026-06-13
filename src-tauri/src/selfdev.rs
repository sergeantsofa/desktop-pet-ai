//! 層次二:半自動「自我修改」。
//!
//! 讓桌寵能讀/改自己的原始碼,但有嚴格護欄:
//! - 總開關 self_dev_enabled(預設關)+ 明確的專案根 self_dev_root
//! - 路徑沙箱:所有檔案操作 canonicalize 後必須落在 root 內,且排除敏感目錄
//! - 寫檔前自動 git 快照(checkpoint),壞了可一鍵還原
//! - 寫檔/還原/跑指令一律經權限卡片確認(在 agent::execute 處理)

use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

/// 禁止觸碰的目錄(相對 root)
const DENY_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", ".claude"];
/// 單檔讀取上限(避免塞爆 context)
const MAX_READ_BYTES: usize = 60_000;

fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    #[cfg(not(windows))]
    let _ = cmd;
}

/// 把相對/絕對路徑解析成「保證在 root 內」的真實路徑。
/// 不存在的檔(write 新檔)會檢查其父目錄。
fn resolve(root: &str, rel: &str) -> Result<PathBuf, String> {
    if root.trim().is_empty() {
        return Err("尚未設定專案根目錄(設定 → 自我修改)".into());
    }
    let root = std::fs::canonicalize(root).map_err(|e| format!("專案根無效:{e}"))?;
    let joined = root.join(rel.trim_start_matches(['/', '\\']));

    // 目標存在就直接 canonicalize;不存在(新檔)則 canonicalize 父目錄再接檔名
    let resolved = if joined.exists() {
        std::fs::canonicalize(&joined).map_err(|e| format!("路徑無效:{e}"))?
    } else {
        let parent = joined.parent().ok_or("路徑無效")?;
        let cano_parent =
            std::fs::canonicalize(parent).map_err(|_| "目標資料夾不存在".to_string())?;
        let name = joined.file_name().ok_or("路徑無效")?;
        cano_parent.join(name)
    };

    if !resolved.starts_with(&root) {
        return Err("超出專案範圍,拒絕存取".into());
    }
    // 檢查相對 root 的第一層是否在黑名單
    if let Ok(rel_path) = resolved.strip_prefix(&root) {
        if let Some(first) = rel_path.components().next() {
            let seg = first.as_os_str().to_string_lossy().to_lowercase();
            if DENY_DIRS.contains(&seg.as_str()) {
                return Err(format!("{seg} 是受保護目錄,拒絕存取"));
            }
        }
    }
    Ok(resolved)
}

pub fn read_file(root: &str, rel: &str) -> Result<String, String> {
    let path = resolve(root, rel)?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("讀取失敗:{e}"))?;
    match text.char_indices().nth(MAX_READ_BYTES) {
        Some((i, _)) => Ok(format!("{}\n…(檔案過長已截斷)", &text[..i])),
        None => Ok(text),
    }
}

pub fn list_dir(root: &str, rel: &str) -> Result<String, String> {
    let dir = resolve(root, if rel.trim().is_empty() { "." } else { rel })?;
    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("讀取目錄失敗:{e}"))?
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .filter(|n| !DENY_DIRS.contains(&n.trim_end_matches('/')))
        .collect();
    entries.sort();
    Ok(entries.join("\n"))
}

/// 寫檔前先做 git 快照(checkpoint),回傳是否有成功 commit。
pub fn git_checkpoint(root: &str, note: &str) -> Result<(), String> {
    git(root, &["add", "-A"])?;
    // 沒有變更時 commit 會失敗,視為正常(無需快照)
    let _ = git(root, &["commit", "-m", &format!("🐾 自我修改快照:{note}")]);
    Ok(())
}

pub fn write_file(root: &str, rel: &str, content: &str) -> Result<String, String> {
    let path = resolve(root, rel)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建立資料夾失敗:{e}"))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("寫入失敗:{e}"))?;
    Ok(format!("已寫入 {rel}({} 字元)", content.chars().count()))
}

/// 還原到上一個 git 快照(救命用)
pub fn git_revert(root: &str) -> Result<String, String> {
    git(root, &["reset", "--hard", "HEAD"])?;
    Ok("已還原到上一個快照。".into())
}

fn git(root: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(root).args(args);
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| format!("git 執行失敗:{e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 跑前端型別檢查當「編譯閘」;回傳結果摘要讓模型自己判讀。
/// (cargo 常不在 PATH,且耗時長;MVP 先用 npm typecheck 驗前端改動)
pub fn run_check(root: &str) -> Result<String, String> {
    let _ = Path::new(root);
    let mut cmd = Command::new("npm");
    cmd.current_dir(root).args(["run", "typecheck"]);
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| format!("npm 執行失敗:{e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok("型別檢查通過 ✅".into())
    } else {
        let tail: String = format!("{stdout}\n{stderr}");
        let tail = tail.trim();
        let start = tail.char_indices().rev().nth(1500).map(|(i, _)| i).unwrap_or(0);
        Err(format!("型別檢查失敗:\n{}", &tail[start..]))
    }
}
