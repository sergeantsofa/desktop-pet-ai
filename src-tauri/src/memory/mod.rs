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

pub fn save(app: &AppHandle, content: &str) -> Result<(), String> {
    with_db(app, |c| {
        c.execute("INSERT INTO memories (content) VALUES (?1)", [content])
            .map(|_| ())
    })
}

pub fn search(app: &AppHandle, keyword: &str, limit: usize) -> Result<Vec<(String, String)>, String> {
    with_db(app, |c| {
        let mut stmt = c.prepare(
            "SELECT content, created_at FROM memories
             WHERE content LIKE ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![format!("%{keyword}%"), limit as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        rows.collect()
    })
}

pub fn forget(app: &AppHandle, keyword: &str) -> Result<usize, String> {
    with_db(app, |c| {
        c.execute(
            "DELETE FROM memories WHERE content LIKE ?1",
            [format!("%{keyword}%")],
        )
    })
}

/// 最近記憶組成的 system prompt 片段;沒有記憶時回空字串
pub fn prompt_section(app: &AppHandle) -> String {
    let Ok(rows) = with_db(app, |c| {
        let mut stmt = c.prepare(
            "SELECT content, created_at FROM memories ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([PROMPT_MEMORIES as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<(String, String)>>>()
    }) else {
        return String::new();
    };
    if rows.is_empty() {
        return String::new();
    }
    let mut section = String::from("\n\n你記得這些事(由舊到新):\n");
    let mut budget = PROMPT_CHAR_BUDGET;
    // DESC 撈出後反轉,呈現由舊到新
    for (content, date) in rows.into_iter().rev() {
        let line = format!("- ({}){}\n", &date[..10.min(date.len())], content);
        if line.chars().count() > budget {
            break;
        }
        budget -= line.chars().count();
        section.push_str(&line);
    }
    section
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

#[tauri::command]
pub fn clear_memories(app: AppHandle) -> Result<usize, String> {
    with_db(&app, |c| c.execute("DELETE FROM memories", []))
}

#[tauri::command]
pub fn clear_history(app: AppHandle) -> Result<usize, String> {
    with_db(&app, |c| c.execute("DELETE FROM messages", []))
}
