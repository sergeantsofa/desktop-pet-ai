//! M3 Agent:工具註冊、執行與權限確認(規格 §6 agent/)。
//!
//! 風險分級:
//! - 唯讀且無隱私疑慮(查時間、系統狀態)→ 自動執行
//! - 隱私敏感(剪貼簿)→ 本地模型自動;雲端模型先問使用者
//! - 有副作用(開網頁)→ 一律先問使用者
//!
//! 確認流程:emit "agent-permission" → 前端顯示允許/拒絕 →
//! agent_permission_response 命令回填(60 秒沒回應視同拒絕)。

use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::oneshot;

/// 等待使用者回應的權限請求(callId → 回填通道)
pub struct PermissionState(pub Mutex<HashMap<String, oneshot::Sender<bool>>>);

/// OpenAI tools 格式的工具規格(隨對話請求送出)。
/// self_dev=true 時額外提供「讀/改自己原始碼」的工具(層次二);
/// spotify=true 時額外提供 Spotify 播放控制工具。
pub fn tool_specs(self_dev: bool, spotify: bool) -> Value {
    let mut specs = base_tool_specs();
    if self_dev {
        if let (Some(arr), Value::Array(dev)) = (specs.as_array_mut(), self_dev_tool_specs()) {
            arr.extend(dev);
        }
    }
    if spotify {
        if let (Some(arr), Value::Array(sp)) = (specs.as_array_mut(), spotify_tool_specs()) {
            arr.extend(sp);
        }
    }
    specs
}

/// Spotify 播放控制工具(僅在 spotify_enabled 時提供)
fn spotify_tool_specs() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "spotify_play",
                "description": "用 Spotify 播放音樂。query 給歌名/歌手就搜尋並播放;query 留空則是「繼續播放」目前暫停的歌。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "要播放的歌名或歌手,例如「周杰倫 稻香」;繼續播放就留空" }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "spotify_pause",
                "description": "暫停 Spotify 目前的播放。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "spotify_next",
                "description": "Spotify 跳到下一首。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "spotify_previous",
                "description": "Spotify 回到上一首。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "spotify_volume",
                "description": "調整 Spotify 音量。給 percent(0~100)就設成絕對音量;或給 direction(up/down)做相對調整(大聲/小聲一點)。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "percent": { "type": "integer", "description": "絕對音量 0~100" },
                        "direction": { "type": "string", "enum": ["up", "down"], "description": "相對調整:up 大聲一點、down 小聲一點" }
                    }
                }
            }
        }
    ])
}

fn base_tool_specs() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "get_time",
                "description": "取得目前的日期、時間與星期。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_clipboard",
                "description": "讀取使用者剪貼簿裡目前的文字內容。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "open_url",
                "description": "用預設瀏覽器幫使用者開啟一個網址(會先徵求使用者同意)。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "完整網址,須以 http:// 或 https:// 開頭" }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "system_status",
                "description": "取得這台電腦目前的 CPU 與記憶體使用狀況。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "save_memory",
                "description": "把一件值得長期記住的事寫進你的記憶(使用者的喜好、身分、約定、重要事件)。一句話、講清楚主詞。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "要記住的事,例如「使用者喜歡貓」" },
                        "importance": { "type": "integer", "description": "重要度 1~5(5=核心身分/長期約定,3=一般偏好,1=瑣事);越高之後越會被優先想起來。預設 3" },
                        "kind": { "type": "string", "enum": ["fact", "preference", "relationship", "event"], "description": "分類:fact 事實 / preference 偏好 / relationship 關係 / event 事件。預設 fact" }
                    },
                    "required": ["content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_memory",
                "description": "搜尋你更久之前的記憶(語意搜尋,會找意思相近的、不只字面;最近的記憶已在系統提示裡)。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "keyword": { "type": "string", "description": "關鍵字" }
                    },
                    "required": ["keyword"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "set_reminder",
                "description": "幫使用者設一個提醒,到時間你會主動跳出來提醒他。in_minutes 和 at_time 擇一提供。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "提醒內容,例如「該去開會了」" },
                        "in_minutes": { "type": "number", "description": "幾分鐘後提醒" },
                        "at_time": { "type": "string", "description": "指定時間,格式 HH:MM 或 YYYY-MM-DD HH:MM;只給 HH:MM 且已過則視為明天" }
                    },
                    "required": ["content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_reminders",
                "description": "列出目前還沒到期的提醒。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "cancel_reminder",
                "description": "取消包含某關鍵字的未到期提醒。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "keyword": { "type": "string", "description": "提醒內容關鍵字" }
                    },
                    "required": ["keyword"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "forget_memory",
                "description": "刪除包含某關鍵字的記憶(使用者要求你忘記某件事時用)。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "keyword": { "type": "string", "description": "要忘掉的記憶關鍵字" }
                    },
                    "required": ["keyword"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "clear_desktop",
                "description": "桌面霸權術:把桌面上其他所有 App 視窗最小化,只留你自己獨佔整個桌面。使用者說「清空桌面」「獨佔桌面」之類時用。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "restore_desktop",
                "description": "把剛剛用桌面霸權術收起來的視窗全部還原歸位。使用者說「復原桌面」「視窗歸位」「把視窗放回來」時用。",
                "parameters": { "type": "object", "properties": {} }
            }
        }
    ])
}

/// 層次二:自我修改工具(僅在 self_dev_enabled 時提供)
fn self_dev_tool_specs() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "dev_list_dir",
                "description": "列出你自己專案某資料夾下的檔案(看自己的程式碼結構)。path 相對於專案根。",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "相對路徑,例如 src 或 src-tauri/src" } }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "dev_read_file",
                "description": "讀取你自己專案的某個原始碼檔(改之前先讀)。",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "相對路徑,例如 src/App.vue" } },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "dev_write_file",
                "description": "改寫你自己專案的某個檔(整個檔覆寫)。會先請使用者同意、自動建立還原點,並在寫入後『自動驗證(前端型別/Rust 編譯/JSON 格式),沒過會自動還原到修改前』。所以一次只改一個檔、確保它自己就能通過編譯;若回報驗證沒過,看錯誤訊息修正後再寫一次。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "相對路徑" },
                        "content": { "type": "string", "description": "檔案的完整新內容" },
                        "summary": { "type": "string", "description": "一句話說明這次為什麼改、改了什麼(會記進成長日誌 self-evolution.md)" }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "dev_run_check",
                "description": "手動跑驗證(前端型別檢查 + Rust 編譯檢查),確認專案目前沒壞掉。dev_write_file 已會自動驗證,這個用在你想再整體確認時。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "dev_revert",
                "description": "把專案還原到上一個 git 還原點(改壞時救命用)。會先請使用者同意。",
                "parameters": { "type": "object", "properties": {} }
            }
        }
    ])
}

/// 工具的中文顯示名(泡泡「(○○中…)」與權限對話框用)
pub fn tool_label(name: &str) -> &'static str {
    match name {
        "get_time" => "查時間",
        "read_clipboard" => "讀剪貼簿",
        "open_url" => "開啟網頁",
        "system_status" => "看系統狀態",
        "save_memory" => "記筆記",
        "search_memory" => "翻記憶",
        "forget_memory" => "忘掉記憶",
        "set_reminder" => "設提醒",
        "list_reminders" => "看提醒",
        "cancel_reminder" => "取消提醒",
        "dev_list_dir" => "看程式碼結構",
        "dev_read_file" => "讀自己的碼",
        "dev_write_file" => "改自己的碼",
        "dev_run_check" => "驗證修改",
        "dev_revert" => "還原修改",
        "spotify_play" => "放音樂",
        "spotify_pause" => "暫停音樂",
        "spotify_next" => "下一首",
        "spotify_previous" => "上一首",
        "spotify_volume" => "調音量",
        "clear_desktop" => "清空桌面",
        "restore_desktop" => "復原桌面",
        _ => "使用工具",
    }
}

/// 執行單一工具呼叫。錯誤也以字串回填,讓模型能向使用者解釋。
/// `cloud`:本輪對話走雲端模型(隱私敏感工具要先徵求同意)。
pub async fn execute(
    app: &AppHandle,
    request_id: &str,
    name: &str,
    args: &Value,
    cloud: bool,
    identity: &str,
) -> String {
    eprintln!("[agent] 工具呼叫:{name} {args}");
    match name {
        "get_time" => get_time(),
        "system_status" => system_status().await,
        "read_clipboard" => {
            if cloud
                && !request_permission(app, request_id, name, "剪貼簿內容會送到雲端模型").await
            {
                return "使用者拒絕了這次剪貼簿存取。".into();
            }
            read_clipboard(app)
        }
        "open_url" => {
            let url = args["url"].as_str().unwrap_or("");
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return "錯誤:網址必須以 http:// 或 https:// 開頭。".into();
            }
            if !request_permission(app, request_id, name, url).await {
                return "使用者拒絕開啟這個網址。".into();
            }
            open_url(app, url)
        }
        "save_memory" => {
            let content = args["content"].as_str().unwrap_or("").trim();
            if content.is_empty() {
                return "錯誤:沒有提供要記住的內容。".into();
            }
            let importance = args["importance"].as_i64().unwrap_or(3);
            let kind = args["kind"].as_str().unwrap_or("fact");
            match crate::memory::remember(app, identity, content, importance, kind).await {
                Ok(()) => {
                    // 新記憶累積到門檻 → 背景重算「她眼中的這個人」
                    tauri::async_runtime::spawn(crate::llm::provider::maybe_refresh_persona(
                        app.clone(),
                        identity.to_string(),
                    ));
                    format!("已記住:{content}")
                }
                Err(e) => format!("記憶寫入失敗:{e}"),
            }
        }
        "search_memory" => {
            let keyword = args["keyword"].as_str().unwrap_or("").trim();
            match crate::memory::recall(app, identity, keyword, 10).await {
                Ok(rows) if rows.is_empty() => format!("沒有找到跟「{keyword}」有關的記憶。"),
                Ok(rows) => rows
                    .iter()
                    .map(|(content, date)| format!("({date}){content}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(e) => format!("搜尋記憶失敗:{e}"),
            }
        }
        "forget_memory" => {
            let keyword = args["keyword"].as_str().unwrap_or("").trim();
            if keyword.is_empty() {
                return "錯誤:沒有提供關鍵字。".into();
            }
            match crate::memory::forget(app, identity, keyword) {
                Ok(0) => format!("本來就沒有跟「{keyword}」有關的記憶。"),
                Ok(n) => format!("已忘掉 {n} 條跟「{keyword}」有關的記憶。"),
                Err(e) => format!("刪除記憶失敗:{e}"),
            }
        }
        "set_reminder" => {
            let content = args["content"].as_str().unwrap_or("").trim();
            if content.is_empty() {
                return "錯誤:沒有提供提醒內容。".into();
            }
            let due_at = match resolve_due(args) {
                Ok(d) => d,
                Err(e) => return e,
            };
            match crate::memory::add_reminder(app, content, &due_at) {
                Ok(()) => format!("提醒已設好:{due_at} → {content}"),
                Err(e) => format!("設提醒失敗:{e}"),
            }
        }
        "list_reminders" => match crate::memory::pending_reminders(app) {
            Ok(rows) if rows.is_empty() => "目前沒有未到期的提醒。".into(),
            Ok(rows) => rows
                .iter()
                .map(|(content, due)| format!("{due} → {content}"))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => format!("讀取提醒失敗:{e}"),
        },
        "cancel_reminder" => {
            let keyword = args["keyword"].as_str().unwrap_or("").trim();
            if keyword.is_empty() {
                return "錯誤:沒有提供關鍵字。".into();
            }
            match crate::memory::cancel_reminders(app, keyword) {
                Ok(0) => format!("沒有找到跟「{keyword}」有關的未到期提醒。"),
                Ok(n) => format!("已取消 {n} 個跟「{keyword}」有關的提醒。"),
                Err(e) => format!("取消提醒失敗:{e}"),
            }
        }
        // ---------- 層次二:自我修改 ----------
        "dev_list_dir" | "dev_read_file" | "dev_write_file" | "dev_run_check" | "dev_revert" => {
            return run_self_dev(app, request_id, name, args).await;
        }
        // ---------- Spotify 播放控制 ----------
        "spotify_play" => crate::spotify::play(app, args["query"].as_str().unwrap_or("")).await,
        "spotify_pause" => crate::spotify::pause(app).await,
        "spotify_next" => crate::spotify::next(app).await,
        "spotify_previous" => crate::spotify::previous(app).await,
        "spotify_volume" => {
            if let Some(p) = args["percent"].as_i64() {
                crate::spotify::set_volume(app, p).await
            } else {
                match args["direction"].as_str() {
                    Some("down") => crate::spotify::adjust_volume(app, -15).await,
                    _ => crate::spotify::adjust_volume(app, 15).await,
                }
            }
        }
        // ---------- 桌面霸權術 ----------
        "clear_desktop" => crate::desktop::clear(app),
        "restore_desktop" => crate::desktop::restore(app),
        other => format!("錯誤:沒有叫做 {other} 的工具。"),
    }
}

/// 自我修改工具的執行(讀/列自動;寫/還原要使用者同意 + git 快照)
async fn run_self_dev(app: &AppHandle, request_id: &str, name: &str, args: &Value) -> String {
    use crate::llm::SettingsState;
    let (enabled, root, auto_verify) = {
        let s = app.state::<SettingsState>();
        let g = s.0.lock().unwrap();
        (g.self_dev_enabled, g.self_dev_root.clone(), g.self_dev_auto_verify)
    };
    if !enabled {
        return "自我修改功能沒有開啟(設定 → 自我修改)。".into();
    }
    let path = args["path"].as_str().unwrap_or("").to_string();

    match name {
        "dev_list_dir" => crate::selfdev::list_dir(&root, &path).unwrap_or_else(|e| format!("錯誤:{e}")),
        "dev_read_file" => crate::selfdev::read_file(&root, &path).unwrap_or_else(|e| format!("錯誤:{e}")),
        "dev_run_check" => {
            let r = root.clone();
            match tauri::async_runtime::spawn_blocking(move || crate::selfdev::verify_change(&r, None)).await {
                Ok(crate::selfdev::CheckOutcome::Pass(m)) => format!("✅ 驗證通過。\n{m}"),
                Ok(crate::selfdev::CheckOutcome::Fail(m)) => format!("❌ 驗證沒過。\n{m}"),
                Ok(crate::selfdev::CheckOutcome::Unavailable(m)) => format!("⚠️ 無法驗證(工具跑不起來)。\n{m}"),
                Err(e) => format!("驗證程序異常:{e}"),
            }
        }
        "dev_write_file" => {
            let content = args["content"].as_str().unwrap_or("");
            if path.is_empty() || content.is_empty() {
                return "錯誤:缺少 path 或 content。".into();
            }
            if !request_permission(app, request_id, name, &format!("改寫檔案:{path}")).await {
                return "使用者拒絕了這次修改。".into();
            }
            // 她可附一句「為什麼改」,寫進成長日誌(self-evolution.md)
            let summary = args["summary"].as_str().unwrap_or("").trim().to_string();
            // 寫前 git 快照(供設定的還原點/時間軸救回)
            let _ = crate::selfdev::git_checkpoint(&root, &path);
            // 寫前單檔快照,供「自動驗證沒過 → 精準還原」用(不動其他未追蹤檔)
            let snap = crate::selfdev::snapshot_file(&root, &path);
            let write_msg = match crate::selfdev::write_file(&root, &path, content) {
                Ok(m) => m,
                Err(e) => return format!("錯誤:{e}"),
            };
            if !auto_verify {
                crate::selfdev::log_evolution(&root, &path, &summary, "未驗證(自動驗證已關)");
                return format!("{write_msg}\n(自動驗證已關閉;記得自己呼叫 dev_run_check 確認沒改壞)");
            }
            // P0 編譯閘:改完自動驗證,沒過自動還原;工具跑不起來(Unavailable)則保留+警告,不誤刪
            let r = root.clone();
            let p = path.clone();
            match tauri::async_runtime::spawn_blocking(move || crate::selfdev::verify_change(&r, Some(&p))).await {
                Ok(crate::selfdev::CheckOutcome::Pass(m)) => {
                    crate::selfdev::log_evolution(&root, &path, &summary, "驗證通過");
                    format!("{write_msg}\n✅ 自動驗證通過:{m}")
                }
                Ok(crate::selfdev::CheckOutcome::Unavailable(why)) => {
                    crate::selfdev::log_evolution(&root, &path, &summary, "未驗證(工具跑不起來,已保留)");
                    format!("{write_msg}\n⚠️ 無法自動驗證({why}),已保留修改;請自行確認或呼叫 dev_run_check。")
                }
                Ok(crate::selfdev::CheckOutcome::Fail(details)) => {
                    let rev = crate::selfdev::restore_file(&root, &path, snap)
                        .map(|_| "已自動還原到修改前。".to_string())
                        .unwrap_or_else(|e| format!("自動還原也失敗了:{e}"));
                    format!(
                        "⚠️ 寫入 {path} 後驗證沒過,{rev}\n{details}\n\
                         (若這是多檔修改的中間步驟、單檔本來就還不能編譯,可到『設定 → 自我修改』關閉「自動驗證並還原」後再試。)"
                    )
                }
                Err(e) => format!("{write_msg}\n⚠️ 驗證程序異常({e}),已保留修改。"),
            }
        }
        "dev_revert" => {
            if !request_permission(app, request_id, name, "把專案還原到上一個快照").await {
                return "使用者拒絕了這次還原。".into();
            }
            crate::selfdev::git_revert(&root).unwrap_or_else(|e| format!("錯誤:{e}"))
        }
        _ => "錯誤:未知的自我修改工具。".into(),
    }
}

/// 解析提醒時間:in_minutes 優先,其次 at_time(HH:MM 已過則視為明天)
fn resolve_due(args: &Value) -> Result<String, String> {
    const FMT: &str = "%Y-%m-%d %H:%M:%S";
    let now = chrono::Local::now();
    if let Some(minutes) = args["in_minutes"].as_f64() {
        if !(0.1..=60.0 * 24.0 * 365.0).contains(&minutes) {
            return Err("錯誤:in_minutes 超出合理範圍。".into());
        }
        let due = now + chrono::Duration::seconds((minutes * 60.0) as i64);
        return Ok(due.format(FMT).to_string());
    }
    if let Some(t) = args["at_time"].as_str() {
        let t = t.trim();
        // 完整日期時間
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M") {
            return Ok(dt.format(FMT).to_string());
        }
        // 只有 HH:MM → 今天;已過 → 明天
        if let Ok(time) = chrono::NaiveTime::parse_from_str(t, "%H:%M") {
            let mut date = now.date_naive();
            if time <= now.time() {
                date += chrono::Duration::days(1);
            }
            return Ok(date.and_time(time).format(FMT).to_string());
        }
        return Err(format!("錯誤:看不懂時間「{t}」,請用 HH:MM 或 YYYY-MM-DD HH:MM。"));
    }
    Err("錯誤:請提供 in_minutes 或 at_time 其中之一。".into())
}

/* ---------------- 個別工具 ---------------- */

fn get_time() -> String {
    let now = chrono::Local::now();
    const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
    let weekday = WEEKDAYS[now.format("%u").to_string().parse::<usize>().unwrap_or(1) - 1];
    format!("現在是 {},星期{weekday}。", now.format("%Y-%m-%d %H:%M:%S"))
}

async fn system_status() -> String {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    // CPU 使用率需要兩次取樣間隔
    tokio::time::sleep(Duration::from_millis(250)).await;
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let cpu = sys.global_cpu_usage();
    let used_gb = sys.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
    let total_gb = sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
    format!("CPU 使用率約 {cpu:.0}%;記憶體用了 {used_gb:.1} GB / {total_gb:.1} GB。")
}

fn read_clipboard(app: &AppHandle) -> String {
    match app.clipboard().read_text() {
        Ok(text) if !text.trim().is_empty() => {
            // 防止超長內容塞爆 context
            const MAX: usize = 2000;
            match text.char_indices().nth(MAX) {
                Some((idx, _)) => format!("剪貼簿內容(過長已截斷):{}", &text[..idx]),
                None => format!("剪貼簿內容:{text}"),
            }
        }
        Ok(_) => "剪貼簿目前是空的(或不是文字)。".into(),
        Err(e) => format!("讀取剪貼簿失敗:{e}"),
    }
}

fn open_url(app: &AppHandle, url: &str) -> String {
    match app.opener().open_url(url, None::<&str>) {
        Ok(()) => format!("已用預設瀏覽器開啟 {url}。"),
        Err(e) => format!("開啟失敗:{e}"),
    }
}

/* ---------------- 權限確認 ---------------- */

/// 向使用者徵求同意;60 秒沒回應視同拒絕。
async fn request_permission(app: &AppHandle, request_id: &str, tool: &str, detail: &str) -> bool {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let call_id = format!("perm-{nanos}");

    let (tx, rx) = oneshot::channel::<bool>();
    app.state::<PermissionState>()
        .0
        .lock()
        .unwrap()
        .insert(call_id.clone(), tx);

    let _ = app.emit(
        "agent-permission",
        json!({
            "requestId": request_id,
            "callId": call_id,
            "tool": tool,
            "label": tool_label(tool),
            "detail": detail,
        }),
    );

    let allowed = matches!(
        tokio::time::timeout(Duration::from_secs(60), rx).await,
        Ok(Ok(true))
    );
    // 逾時的話清掉殘留的通道;前端收到 close 事件收起卡片
    app.state::<PermissionState>().0.lock().unwrap().remove(&call_id);
    let _ = app.emit("agent-permission-close", json!({ "callId": call_id }));
    allowed
}

#[tauri::command]
pub fn agent_permission_response(state: State<PermissionState>, call_id: String, allow: bool) {
    if let Some(tx) = state.0.lock().unwrap().remove(&call_id) {
        let _ = tx.send(allow);
    }
}
