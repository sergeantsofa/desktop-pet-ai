//! M4:長期記憶(SQLite,規格 §6 memory/)。
//!
//! 兩張表:
//! - memories:蒸餾過的事實(模型透過 save_memory 工具寫入),
//!   最近 N 條會注入 system prompt;更舊的用 search_memory 工具撈。
//! - messages:對話紀錄,重啟後載入最近幾輪讓對話接得上。
//!
//! 向量檢索(語意搜尋)留待 M4.5;桌寵記憶量級下 LIKE + 全量注入已夠用。

use crate::llm::ChatMessage;
use rusqlite::Connection;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

pub struct MemoryDb(pub Mutex<Connection>);

/// 注入 system prompt 的記憶條數與字元上限
const PROMPT_MEMORIES: usize = 30;
const PROMPT_CHAR_BUDGET: usize = 1500;

pub fn init(app: &AppHandle) -> Result<MemoryDb, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let conn = Connection::open(dir.join("memory.db")).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
         );
         CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
         );
         CREATE TABLE IF NOT EXISTS reminders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            due_at TEXT NOT NULL,
            fired INTEGER NOT NULL DEFAULT 0
         );",
    )
    .map_err(|e| e.to_string())?;
    // 漸進式 migration(P2):舊 DB 沒有這些欄位 → 補上(已存在會回錯,忽略)
    let _ = conn.execute("ALTER TABLE memories ADD COLUMN embedding TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE memories ADD COLUMN importance INTEGER NOT NULL DEFAULT 3",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE memories ADD COLUMN kind TEXT NOT NULL DEFAULT 'fact'",
        [],
    );
    // P1 記憶分區:scope = 'owner'(桌面主人)/ 'shared'(共用)/ 帳號 id(網頁各自)。
    // 舊資料一律歸 owner(都是關於主人的);回話範圍 = shared + 當前身分。
    let _ = conn.execute(
        "ALTER TABLE memories ADD COLUMN scope TEXT NOT NULL DEFAULT 'owner'",
        [],
    );
    // 小型鍵值表:存人格摘要等(key → value)
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
         );
         CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT UNIQUE NOT NULL,
            salt TEXT NOT NULL,
            hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
         );
         CREATE TABLE IF NOT EXISTS invite_codes (
            code TEXT PRIMARY KEY,
            used_by TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
         );",
    );
    Ok(MemoryDb(Mutex::new(conn)))
}

fn with_db<T>(
    app: &AppHandle,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> Result<T, String> {
    let db = app
        .try_state::<MemoryDb>()
        .ok_or("記憶資料庫未就緒")?;
    let conn = db.0.lock().unwrap();
    f(&conn).map_err(|e| e.to_string())
}

/* ---------------- 記憶(工具用) ---------------- */

/// 寫入一條長期記憶。順手用 Ollama 算 embedding 供語意檢索;算不出來就存純文字
/// (該條之後的檢索會退回關鍵字)。importance 1~5,影響注入 system prompt 的優先序。
pub async fn remember(
    app: &AppHandle,
    identity: &str,
    content: &str,
    importance: i64,
    kind: &str,
) -> Result<(), String> {
    let importance = importance.clamp(1, 5);
    let kind = normalize_kind(kind);
    let scope = scope_of(identity);
    let emb_json = embed(app, content)
        .await
        .and_then(|v| serde_json::to_string(&v).ok());
    with_db(app, |c| {
        c.execute(
            "INSERT INTO memories (content, importance, kind, embedding, scope) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![content, importance, kind, emb_json, scope],
        )
        .map(|_| ())
    })
}

/// 把 identity 正規化成 scope 字串(空 → owner;不允許冒用保留字 shared)。
fn scope_of(identity: &str) -> String {
    let id = identity.trim();
    if id.is_empty() || id == "shared" {
        "owner".to_string()
    } else {
        id.to_string()
    }
}

/// 把 kind 正規化成四類之一(中英皆可),其餘當 fact。
fn normalize_kind(k: &str) -> &'static str {
    match k.trim().to_lowercase().as_str() {
        "preference" | "偏好" => "preference",
        "relationship" | "關係" | "关系" => "relationship",
        "event" | "事件" => "event",
        _ => "fact",
    }
}

/// 用 Ollama 的 embeddings 端點把文字轉向量;任何失敗都回 None(語意檢索退回關鍵字)。
/// 沿用設定裡 ollama provider 的 base_url + `embed_model`(預設 nomic-embed-text)。
async fn embed(app: &AppHandle, text: &str) -> Option<Vec<f32>> {
    let (base, model) = {
        let s = app.try_state::<crate::llm::SettingsState>()?;
        let g = s.0.lock().unwrap();
        let base = g
            .providers
            .iter()
            .find(|p| p.id == "ollama")
            .map(|p| p.base_url.clone())?;
        (base, g.embed_model.clone())
    };
    if model.trim().is_empty() {
        return None;
    }
    let url = format!("{}/embeddings", base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "model": model, "input": text }))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    let arr = v["data"][0]["embedding"].as_array()?;
    let vec: Vec<f32> = arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect();
    if vec.is_empty() {
        None
    } else {
        Some(vec)
    }
}

/// 餘弦相似度(長度不同就取較短的,不會 panic)。
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

pub fn search(
    app: &AppHandle,
    identity: &str,
    keyword: &str,
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    let scope = scope_of(identity);
    with_db(app, |c| {
        let mut stmt = c.prepare(
            "SELECT content, created_at FROM memories
             WHERE content LIKE ?1 AND scope IN ('shared', ?2) ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![format!("%{keyword}%"), scope, limit as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        rows.collect()
    })
}

/// 語意檢索:用 query 的向量找最相關的記憶;算不出向量或沒有已嵌入的記憶 → 退回關鍵字搜尋。
pub async fn recall(
    app: &AppHandle,
    identity: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    let scope = scope_of(identity);
    if let Some(q) = embed(app, query).await {
        let rows: Vec<(String, String, String, i64)> = with_db(app, |c| {
            let mut stmt = c.prepare(
                "SELECT content, created_at, embedding, importance FROM memories
                 WHERE embedding IS NOT NULL AND scope IN ('shared', ?1)",
            )?;
            let rows = stmt.query_map([&scope], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        if !rows.is_empty() {
            let now = chrono::Local::now().naive_local();
            // 綜合評分:語意相似度 × 重要度權重 × 時間衰減(老但重要的仍找得到)
            let mut scored: Vec<(f32, String, String)> = rows
                .into_iter()
                .filter_map(|(content, date, emb, imp)| {
                    let v: Vec<f32> = serde_json::from_str(&emb).ok()?;
                    let imp_w = 0.6 + 0.1 * imp.clamp(1, 5) as f32; // 1→0.7 .. 5→1.1
                    let score = cosine(&q, &v) * imp_w * recency_weight(&date, now);
                    Some((score, content, date))
                })
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            return Ok(scored
                .into_iter()
                .take(limit)
                .map(|(_, c, d)| (c, d))
                .collect());
        }
    }
    // 沒向量可用 → 退回關鍵字
    search(app, identity, query, limit)
}

/// 時間衰減權重:越新越接近 1,越舊趨近 0.4 下限(60 天 e 折,老記憶不會完全消失)。
fn recency_weight(date: &str, now: chrono::NaiveDateTime) -> f32 {
    match chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S") {
        Ok(t) => {
            let days = (now - t).num_days().max(0) as f32;
            0.4 + 0.6 * (-days / 60.0).exp()
        }
        Err(_) => 1.0,
    }
}

pub fn forget(app: &AppHandle, identity: &str, keyword: &str) -> Result<usize, String> {
    let scope = scope_of(identity);
    with_db(app, |c| {
        c.execute(
            "DELETE FROM memories WHERE content LIKE ?1 AND scope = ?2",
            rusqlite::params![format!("%{keyword}%"), scope],
        )
    })
}

/* ---------------- 遊戲知識庫(scope='game',與個人記憶分開) ---------------- */

#[derive(serde::Serialize)]
pub struct GameKnowledge {
    pub id: i64,
    pub content: String,
}

/// 教一條遊戲知識(存 scope='game',順手算 embedding 供語意檢索)。
#[tauri::command]
pub async fn gamekb_add(app: AppHandle, text: String) -> Result<i64, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("內容不可空白".into());
    }
    let emb_json = embed(&app, &text).await.map(|v| serde_json::to_string(&v).unwrap_or_default());
    with_db(&app, |c| {
        c.execute(
            "INSERT INTO memories (content, importance, kind, embedding, scope) VALUES (?1, 3, 'fact', ?2, 'game')",
            rusqlite::params![text, emb_json],
        )?;
        Ok(c.last_insert_rowid())
    })
}

/// 列出所有遊戲知識(管理用)。
#[tauri::command]
pub fn gamekb_list(app: AppHandle) -> Result<Vec<GameKnowledge>, String> {
    with_db(&app, |c| {
        let mut stmt =
            c.prepare("SELECT id, content FROM memories WHERE scope = 'game' ORDER BY id DESC")?;
        let rows = stmt
            .query_map([], |r| Ok(GameKnowledge { id: r.get(0)?, content: r.get(1)? }))?;
        rows.collect()
    })
}

/// 刪一條遊戲知識。
#[tauri::command]
pub fn gamekb_delete(app: AppHandle, id: i64) -> Result<usize, String> {
    with_db(&app, |c| {
        c.execute("DELETE FROM memories WHERE id = ?1 AND scope = 'game'", rusqlite::params![id])
    })
}

/// 語意檢索遊戲知識(只在 scope='game');算不出向量就退回關鍵字。
pub async fn gamekb_recall(app: &AppHandle, query: &str, limit: usize) -> Vec<String> {
    if let Some(q) = embed(app, query).await {
        let rows: Result<Vec<(String, String)>, String> = with_db(app, |c| {
            let mut stmt = c.prepare(
                "SELECT content, embedding FROM memories WHERE embedding IS NOT NULL AND scope = 'game'",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect()
        });
        if let Ok(rows) = rows {
            if !rows.is_empty() {
                let mut scored: Vec<(f32, String)> = rows
                    .into_iter()
                    .filter_map(|(content, emb)| {
                        let v: Vec<f32> = serde_json::from_str(&emb).ok()?;
                        Some((cosine(&q, &v), content))
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                return scored.into_iter().take(limit).map(|(_, c)| c).collect();
            }
        }
    }
    // 退回關鍵字(知識可能沒嵌入向量);抓不到就回最近幾條
    with_db(app, |c| {
        let mut stmt = c.prepare(
            "SELECT content FROM memories WHERE scope = 'game' AND content LIKE ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![format!("%{query}%"), limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        rows.collect::<rusqlite::Result<Vec<String>>>()
    })
    .unwrap_or_default()
}

/* ---------------- 遠端每位使用者各自的設定(存 meta:usersettings:<帳號>) ---------------- */

/// 某帳號的個人設定(JSON);沒存過 → 回預設值。
pub fn user_settings(app: &AppHandle, username: &str) -> serde_json::Value {
    if let Some(s) = get_meta(app, &format!("usersettings:{username}")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if v.is_object() {
                return v;
            }
        }
    }
    serde_json::json!({
        "charScale": 1.0,
        "bubbleStyle": "classic",
        "personaPrompt": "",
        "ttsVoice": "zh-CN-XiaoyiNeural",
        "ttsRate": 1.0,
        "gameKb": false
    })
}

/// 存某帳號的個人設定(整包 JSON 字串)。
pub fn set_user_settings(app: &AppHandle, username: &str, json: &str) -> Result<(), String> {
    set_meta(app, &format!("usersettings:{username}"), json)
}

/// 最近記憶組成的 system prompt 片段;沒有記憶時回空字串
pub fn prompt_section(app: &AppHandle, identity: &str) -> String {
    let scope = scope_of(identity);
    // 先挑「重要 + 最近」的前 N 條(importance 高的老記憶也不會被新瑣事擠掉),範圍 = shared + 當前身分
    let Ok(mut rows) = with_db(app, |c| {
        let mut stmt = c.prepare(
            "SELECT content, created_at FROM memories
             WHERE scope IN ('shared', ?1) ORDER BY importance DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![scope, PROMPT_MEMORIES as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<(String, String)>>>()
    }) else {
        return String::new();
    };
    if rows.is_empty() {
        return String::new();
    }
    // 選出後依時間由舊到新呈現(created_at 是可排序的 "YYYY-MM-DD HH:MM:SS")
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    let mut section = String::from("\n\n你記得這些事(由舊到新):\n");
    let mut budget = PROMPT_CHAR_BUDGET;
    for (content, date) in rows {
        let line = format!("- ({}){}\n", &date[..10.min(date.len())], content);
        if line.chars().count() > budget {
            break;
        }
        budget -= line.chars().count();
        section.push_str(&line);
    }
    section
}

/// 注入對話的完整記憶上下文:人格摘要(她眼中的這個人)+ 該身分可見的記憶(shared + 自己)。
pub fn context_section(app: &AppHandle, identity: &str) -> String {
    let mut s = String::new();
    if let Some(sum) = get_persona_summary(app, identity) {
        let sum = sum.trim();
        if !sum.is_empty() {
            s.push_str(&format!("\n\n(你對對方的長期印象:{sum})"));
        }
    }
    s.push_str(&prompt_section(app, identity));
    s
}

/* ---------------- meta / 人格摘要(P2) ---------------- */

pub fn memory_count(app: &AppHandle, identity: &str) -> i64 {
    let scope = scope_of(identity);
    with_db(app, |c| {
        c.query_row(
            "SELECT COUNT(*) FROM memories WHERE scope IN ('shared', ?1)",
            [&scope],
            |r| r.get(0),
        )
    })
    .unwrap_or(0)
}

/// 人格摘要 / 計數的 meta key(每個身分一份)
pub fn persona_key(identity: &str) -> String {
    format!("persona_summary:{}", scope_of(identity))
}
pub fn persona_count_key(identity: &str) -> String {
    format!("mem_count_at_summary:{}", scope_of(identity))
}

pub fn set_meta(app: &AppHandle, key: &str, value: &str) -> Result<(), String> {
    with_db(app, |c| {
        c.execute(
            "INSERT INTO meta (key, value, updated_at) VALUES (?1, ?2, datetime('now','localtime'))
             ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = datetime('now','localtime')",
            rusqlite::params![key, value],
        )
        .map(|_| ())
    })
}

pub fn get_meta(app: &AppHandle, key: &str) -> Option<String> {
    with_db(app, |c| {
        c.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| {
            r.get::<_, String>(0)
        })
    })
    .ok()
}

pub fn get_persona_summary(app: &AppHandle, identity: &str) -> Option<String> {
    get_meta(app, &persona_key(identity))
}

/// 取某身分「重要 + 最近」的記憶文字(shared + 自己),供人格摘要蒸餾用。
pub fn recent_memory_texts(app: &AppHandle, identity: &str, limit: usize) -> Vec<String> {
    let scope = scope_of(identity);
    with_db(app, |c| {
        let mut stmt = c.prepare(
            "SELECT content FROM memories
             WHERE scope IN ('shared', ?1) ORDER BY importance DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![scope, limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        rows.collect::<rusqlite::Result<Vec<String>>>()
    })
    .unwrap_or_default()
}

/// 把過去沒有向量的舊記憶補算 embedding(設定面板的「回填」按鈕用);回傳補了幾條。
#[tauri::command]
pub async fn backfill_embeddings(app: AppHandle) -> Result<usize, String> {
    let pending: Vec<(i64, String)> = with_db(&app, |c| {
        let mut stmt = c.prepare("SELECT id, content FROM memories WHERE embedding IS NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;
    let mut n = 0;
    for (id, content) in pending {
        if let Some(v) = embed(&app, &content).await {
            if let Ok(j) = serde_json::to_string(&v) {
                let _ = with_db(&app, |c| {
                    c.execute(
                        "UPDATE memories SET embedding = ?1 WHERE id = ?2",
                        rusqlite::params![j, id],
                    )
                });
                n += 1;
            }
        }
    }
    Ok(n)
}

/* ---------------- 提醒(M4.5,scheduler 與工具用) ---------------- */

pub fn add_reminder(app: &AppHandle, content: &str, due_at: &str) -> Result<(), String> {
    with_db(app, |c| {
        c.execute(
            "INSERT INTO reminders (content, due_at) VALUES (?1, ?2)",
            [content, due_at],
        )
        .map(|_| ())
    })
}

/// 取出所有到期未觸發的提醒並標記為已觸發(原子操作,避免重複跳)
pub fn take_due_reminders(app: &AppHandle, now: &str) -> Vec<String> {
    with_db(app, |c| {
        let mut stmt = c.prepare(
            "SELECT id, content FROM reminders WHERE fired = 0 AND due_at <= ?1 ORDER BY due_at",
        )?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([now], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, _) in &rows {
            c.execute("UPDATE reminders SET fired = 1 WHERE id = ?1", [id])?;
        }
        Ok(rows.into_iter().map(|(_, content)| content).collect())
    })
    .unwrap_or_default()
}

pub fn pending_reminders(app: &AppHandle) -> Result<Vec<(String, String)>, String> {
    with_db(app, |c| {
        let mut stmt = c.prepare(
            "SELECT content, due_at FROM reminders WHERE fired = 0 ORDER BY due_at LIMIT 20",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect()
    })
}

pub fn cancel_reminders(app: &AppHandle, keyword: &str) -> Result<usize, String> {
    with_db(app, |c| {
        c.execute(
            "DELETE FROM reminders WHERE fired = 0 AND content LIKE ?1",
            [format!("%{keyword}%")],
        )
    })
}

/* ---------------- 對話紀錄 ---------------- */

pub fn log_message(app: &AppHandle, role: &str, content: &str) {
    if content.trim().is_empty() {
        return;
    }
    let _ = with_db(app, |c| {
        c.execute(
            "INSERT INTO messages (role, content) VALUES (?1, ?2)",
            [role, content],
        )
    });
}

/* ---------------- Tauri 命令 ---------------- */

/// 載入最近的對話紀錄(重啟接續上下文;limit = 保留輪數 * 2)
#[tauri::command]
pub fn load_recent_history(
    app: AppHandle,
    state: State<crate::llm::SettingsState>,
) -> Vec<ChatMessage> {
    let limit = state.0.lock().unwrap().context_turns.max(1) * 2;
    with_db(&app, |c| {
        let mut stmt = c.prepare(
            "SELECT role, content FROM (
                SELECT id, role, content FROM messages ORDER BY id DESC LIMIT ?1
             ) ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(ChatMessage {
                role: r.get(0)?,
                content: r.get(1)?,
            })
        })?;
        rows.collect()
    })
    .unwrap_or_default()
}

/// 清空主人(owner)的長期記憶。網頁帳號各自的記憶不在此清(留待帳號管理)。
#[tauri::command]
pub fn clear_memories(app: AppHandle) -> Result<usize, String> {
    with_db(&app, |c| {
        c.execute("DELETE FROM memories WHERE scope = 'owner'", [])
    })
}

#[tauri::command]
pub fn clear_history(app: AppHandle) -> Result<usize, String> {
    with_db(&app, |c| c.execute("DELETE FROM messages", []))
}
