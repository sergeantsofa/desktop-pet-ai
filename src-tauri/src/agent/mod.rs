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

/// OpenAI tools 格式的工具規格(隨對話請求送出)
pub fn tool_specs() -> Value {
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
                        "content": { "type": "string", "description": "要記住的事,例如「使用者喜歡貓」" }
                    },
                    "required": ["content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_memory",
                "description": "用關鍵字搜尋你更久之前的記憶(最近的記憶已在系統提示裡,不用搜)。",
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
) -> String {
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
            match crate::memory::save(app, content) {
                Ok(()) => format!("已記住:{content}"),
                Err(e) => format!("記憶寫入失敗:{e}"),
            }
        }
        "search_memory" => {
            let keyword = args["keyword"].as_str().unwrap_or("").trim();
            match crate::memory::search(app, keyword, 10) {
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
            match crate::memory::forget(app, keyword) {
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
        other => format!("錯誤:沒有叫做 {other} 的工具。"),
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
