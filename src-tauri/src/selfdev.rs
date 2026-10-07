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

use tauri::Manager;

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

/// 禁止觸碰的目錄(相對 root)
const DENY_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", ".claude"];
/// 單檔讀取上限(避免塞爆 context)
const MAX_READ_BYTES: usize = 60_000;

/// 禁止「她自己改」的關鍵檔(比對相對路徑結尾):護欄/沙箱/驗證閘、金鑰、工具權限核心。
/// 不讓她拆掉自己的安全機制(例如改 selfdev.rs 把驗證門關掉)。要動這些請由人手動編輯。
const DENY_WRITE_FILES: &[&str] = &[
    "src-tauri/src/selfdev.rs",   // 護欄/沙箱/驗證閘本身
    "src-tauri/src/llm/keys.rs",  // 金鑰處理
    "src-tauri/src/agent/mod.rs", // 工具權限 + run_self_dev(驗證閘的呼叫端)
];

/// 若 rel 命中受保護檔,回傳該檔(供錯誤訊息);否則 None。
fn write_protected(rel: &str) -> Option<&'static str> {
    let lp = rel.replace('\\', "/").to_lowercase();
    DENY_WRITE_FILES.iter().copied().find(|p| lp.ends_with(*p))
}

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
    if let Some(p) = write_protected(rel) {
        return Err(format!(
            "{p} 是受保護檔(自我修改的護欄/金鑰/工具權限核心),不允許自我修改。要改請由人手動編輯。"
        ));
    }
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

/* ---------------- 自動驗證閘(P0:改完自動驗證,沒過自動還原) ---------------- */

/// 驗證結果三態。關鍵:把「工具跑不起來(Unavailable)」與「檢查真的有錯(Fail)」分開,
/// 否則環境問題(cargo/npm 不在 PATH)會被誤判成失敗,把好的修改也還原掉。
#[derive(Debug)]
pub enum CheckOutcome {
    Pass(String),
    Fail(String),
    Unavailable(String),
}

/// 依「被改的檔」決定要跑哪個編譯閘並執行;changed=None 時前端+後端都跑(dev_run_check 用)。
pub fn verify_change(root: &str, changed: Option<&str>) -> CheckOutcome {
    let (want_front, want_rust) = match changed {
        None => (true, true),
        Some(p) => {
            let lp = p.replace('\\', "/").to_lowercase();
            // 設定/資料檔沒有編譯器:用 JSON 解析當驗證(常見「把 characters.json 改壞」)
            if lp.ends_with(".json") {
                return validate_json(root, p);
            }
            let rust = lp.ends_with(".rs") || lp.contains("src-tauri/");
            let front = lp.ends_with(".ts")
                || lp.ends_with(".tsx")
                || lp.ends_with(".vue")
                || lp.ends_with(".js")
                || lp.ends_with(".jsx")
                || lp.ends_with(".mjs")
                || lp.starts_with("src/");
            if !rust && !front {
                return CheckOutcome::Pass(format!("{p}:無對應編譯檢查,略過驗證"));
            }
            (front, rust)
        }
    };

    let mut details = String::new();
    let mut any_fail = false;
    let mut any_pass = false;
    let mut any_unavail = false;

    if want_front {
        match run_typecheck(root) {
            CheckOutcome::Pass(m) => { any_pass = true; details.push_str(&format!("前端型別檢查{m}\n")); }
            CheckOutcome::Fail(m) => { any_fail = true; details.push_str(&format!("{m}\n")); }
            CheckOutcome::Unavailable(m) => { any_unavail = true; details.push_str(&format!("前端檢查略過({m})\n")); }
        }
    }
    if want_rust {
        match run_cargo_check(root) {
            CheckOutcome::Pass(m) => { any_pass = true; details.push_str(&format!("Rust 編譯檢查{m}\n")); }
            CheckOutcome::Fail(m) => { any_fail = true; details.push_str(&format!("{m}\n")); }
            CheckOutcome::Unavailable(m) => { any_unavail = true; details.push_str(&format!("Rust 檢查略過({m})\n")); }
        }
    }

    let details = details.trim().to_string();
    if any_fail {
        CheckOutcome::Fail(details)
    } else if any_pass {
        CheckOutcome::Pass(details)
    } else if any_unavail {
        CheckOutcome::Unavailable(details)
    } else {
        CheckOutcome::Pass("(沒有要檢查的項目)".into())
    }
}

/// 跑一個指令確認「跑得起來」(用於工具可用性探測:跑不起來 → Unavailable,不還原)
fn runs_ok(mut cmd: Command) -> bool {
    matches!(cmd.output(), Ok(o) if o.status.success())
}

/// 取 stdout+stderr 的尾端 n 字元(錯誤訊息只留尾段,避免塞爆 context)
fn tail(out: &std::process::Output, n: usize) -> String {
    let s = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let s = s.trim();
    match s.char_indices().rev().nth(n) {
        Some((i, _)) => s[i..].to_string(),
        None => s.to_string(),
    }
}

/// 組 npm 指令(Windows 上 npm 是 npm.cmd,要走 `cmd /C` 才找得到)
fn npm_cmd(root: &str, args: &[&str]) -> Command {
    let mut cmd;
    #[cfg(windows)]
    {
        cmd = Command::new("cmd");
        cmd.arg("/C").arg("npm").args(args);
    }
    #[cfg(not(windows))]
    {
        cmd = Command::new("npm");
        cmd.args(args);
    }
    cmd.current_dir(root);
    hide_console(&mut cmd);
    cmd
}

fn run_typecheck(root: &str) -> CheckOutcome {
    if !runs_ok(npm_cmd(root, &["--version"])) {
        return CheckOutcome::Unavailable("找不到 npm".into());
    }
    match npm_cmd(root, &["run", "typecheck"]).output() {
        Ok(out) if out.status.success() => CheckOutcome::Pass("通過 ✅".into()),
        Ok(out) => CheckOutcome::Fail(format!("型別檢查失敗:\n{}", tail(&out, 1500))),
        Err(e) => CheckOutcome::Unavailable(format!("npm 執行失敗:{e}")),
    }
}

fn run_cargo_check(root: &str) -> CheckOutcome {
    let dir = Path::new(root).join("src-tauri");
    if !dir.exists() {
        return CheckOutcome::Unavailable("找不到 src-tauri".into());
    }
    let mut probe = Command::new("cargo");
    probe.current_dir(&dir).arg("--version");
    hide_console(&mut probe);
    if !runs_ok(probe) {
        return CheckOutcome::Unavailable("找不到 cargo(PATH 沒有 ~/.cargo/bin?)".into());
    }
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&dir).args(["check", "--message-format", "short"]);
    hide_console(&mut cmd);
    match cmd.output() {
        Ok(out) if out.status.success() => CheckOutcome::Pass("通過 ✅".into()),
        Ok(out) => CheckOutcome::Fail(format!("Rust 編譯失敗:\n{}", tail(&out, 1500))),
        Err(e) => CheckOutcome::Unavailable(format!("cargo 執行失敗:{e}")),
    }
}

/// JSON 檔的驗證:能不能 parse(抓「她把設定/資料檔改成壞 JSON」)
fn validate_json(root: &str, rel: &str) -> CheckOutcome {
    let path = match resolve(root, rel) {
        Ok(p) => p,
        Err(e) => return CheckOutcome::Unavailable(format!("路徑無效:{e}")),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return CheckOutcome::Unavailable(format!("讀取失敗:{e}")),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(_) => CheckOutcome::Pass(format!("{rel}:JSON 格式正確 ✅")),
        Err(e) => CheckOutcome::Fail(format!("{rel}:JSON 格式錯誤 — {e}")),
    }
}

/// 寫檔前的單檔快照(供「自動驗證沒過 → 精準還原」用,不動到其他未追蹤檔)。
/// 回傳 None 代表此檔原本不存在(新檔)。
pub fn snapshot_file(root: &str, rel: &str) -> Option<Vec<u8>> {
    let path = resolve(root, rel).ok()?;
    std::fs::read(&path).ok()
}

/// 依快照精準還原單一檔:Some→寫回舊內容;None→刪掉(新檔)。
pub fn restore_file(root: &str, rel: &str, snap: Option<Vec<u8>>) -> Result<(), String> {
    let path = resolve(root, rel)?;
    match snap {
        Some(bytes) => std::fs::write(&path, bytes).map_err(|e| format!("還原寫回失敗:{e}")),
        None => {
            let _ = std::fs::remove_file(&path);
            Ok(())
        }
    }
}

/* ---------------- 成長日誌 / 還原點(P1) ---------------- */

/// 把一次「保留下來」的自我修改記進成長日誌(root/self-evolution.md;會被下次快照一起 commit)。
/// 最新的記在最下面。失敗就靜默略過(日誌不該擋住主流程)。
pub fn log_evolution(root: &str, file: &str, summary: &str, result: &str) {
    let root_path = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(_) => return,
    };
    let log = root_path.join("self-evolution.md");
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M");
    let summary = if summary.trim().is_empty() { "(她沒說原因)" } else { summary.trim() };
    let mut content = std::fs::read_to_string(&log).unwrap_or_default();
    if content.is_empty() {
        content.push_str(
            "# 霓璃的成長日誌\n\n> 她每次成功改自己時自動記一行:改了什麼、為什麼、驗證結果。最新在最下面。\n\n",
        );
    }
    content.push_str(&format!("- **{stamp}** `{file}` — {summary}({result})\n"));
    let _ = std::fs::write(&log, content);
}

/// 一個 git 還原點(自我修改快照或一般 commit)
#[derive(Debug, serde::Serialize)]
pub struct Checkpoint {
    pub sha: String,
    pub date: String,
    pub note: String,
}

/// 列出最近的還原點(commit),供設定面板的「時間軸」用。
pub fn list_checkpoints(root: &str) -> Result<Vec<Checkpoint>, String> {
    let out = git(root, &["log", "-n", "40", "--pretty=format:%h%x09%ci%x09%s"])?;
    let mut v = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(3, '\t');
        let sha = parts.next().unwrap_or("").trim().to_string();
        let date = parts.next().unwrap_or("").trim().to_string();
        let note = parts.next().unwrap_or("").trim().to_string();
        if !sha.is_empty() {
            v.push(Checkpoint { sha, date, note });
        }
    }
    Ok(v)
}

/// 還原到指定還原點。還原前先自動快照現狀(時光旅行後回得來);sha 須為十六進位。
pub fn restore_checkpoint(root: &str, sha: &str) -> Result<String, String> {
    let sha = sha.trim();
    if sha.is_empty() || sha.len() > 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("無效的還原點代碼。".into());
    }
    let _ = git_checkpoint(root, "還原前自動快照");
    git(root, &["reset", "--hard", sha])?;
    Ok(format!("已還原到還原點 {sha}(還原前的狀態也已存成一個快照,可再還原回來)。"))
}

/* ---------------- Tauri 命令(設定面板的還原點面板用) ---------------- */

fn dev_root(app: &tauri::AppHandle) -> Result<String, String> {
    let s = app.state::<crate::llm::SettingsState>();
    let root = s.0.lock().unwrap().self_dev_root.clone();
    if root.trim().is_empty() {
        Err("尚未設定自我修改的專案根目錄(設定 → 自我修改)。".into())
    } else {
        Ok(root)
    }
}

#[tauri::command]
pub fn dev_list_checkpoints(app: tauri::AppHandle) -> Result<Vec<Checkpoint>, String> {
    list_checkpoints(&dev_root(&app)?)
}

#[tauri::command]
pub fn dev_restore_checkpoint(app: tauri::AppHandle, sha: String) -> Result<String, String> {
    restore_checkpoint(&dev_root(&app)?, &sha)
}
