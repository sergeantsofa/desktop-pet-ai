//! Phase 2:網頁帳號認證(帳號密碼存本機 DB,owner 自己管)。
//!
//! - 密碼:隨機 salt + 多輪 SHA-256 stretching。本機區網、信任使用者的場景足夠;
//!   不是 bcrypt/argon2 等級,但避免明文/單次雜湊,且不額外加相依。
//! - session token:記憶體 map(`AuthState`),重啟後需重新登入(可接受)。
//! - 帳號由桌面主人在「設定」建立;網頁端只能登入,不能自行註冊(= 自己管)。
//! - users 表建立在 memory::init(與其他表同一個 memory.db)。

use std::{collections::HashMap, sync::Mutex};

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::memory::MemoryDb;

/// token → username 的記憶體 session 表(managed state)
#[derive(Default)]
pub struct AuthState(pub Mutex<HashMap<String, String>>);

/// 密碼 stretching 輪數(越高越慢但越難暴力破解)
const STRETCH_ROUNDS: u32 = 100_000;
/// 帳號保留字(會跟記憶 scope 的特殊值衝突)
const RESERVED: &[&str] = &["owner", "shared"];

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    let _ = getrandom::getrandom(&mut buf);
    to_hex(&buf)
}

/// salt(hex)+ 多輪 SHA-256 → hash(hex)
fn stretch(password: &str, salt_hex: &str) -> String {
    let mut data = format!("{salt_hex}:{password}").into_bytes();
    for _ in 0..STRETCH_ROUNDS {
        let mut h = Sha256::new();
        h.update(&data);
        data = h.finalize().to_vec();
    }
    to_hex(&data)
}

/// 常數時間比較(避免時序側錄)
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn db<T>(
    app: &AppHandle,
    f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
) -> Result<T, String> {
    let state = app.try_state::<MemoryDb>().ok_or("記憶資料庫未就緒")?;
    let conn = state.0.lock().unwrap();
    f(&conn).map_err(|e| e.to_string())
}

/* ---------------- 帳號管理(owner 用) ---------------- */

pub fn create_user(app: &AppHandle, username: &str, password: &str) -> Result<(), String> {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return Err("帳號與密碼都要填".into());
    }
    if username.len() > 40 || RESERVED.contains(&username.to_lowercase().as_str()) {
        return Err("這個帳號名稱不可用(保留字或過長)".into());
    }
    let salt = random_hex(16);
    let hash = stretch(password, &salt);
    db(app, |c| {
        c.execute(
            "INSERT INTO users (username, salt, hash) VALUES (?1, ?2, ?3)",
            rusqlite::params![username, salt, hash],
        )
        .map(|_| ())
    })
    .map_err(|e| {
        if e.contains("UNIQUE") {
            "帳號已存在".into()
        } else {
            e
        }
    })
}

pub fn verify_login(app: &AppHandle, username: &str, password: &str) -> bool {
    let row = db(app, |c| {
        c.query_row(
            "SELECT salt, hash FROM users WHERE username = ?1",
            [username.trim()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
    });
    match row {
        Ok((salt, hash)) => ct_eq(&stretch(password, &salt), &hash),
        Err(_) => false,
    }
}

pub fn list_users(app: &AppHandle) -> Vec<String> {
    db(app, |c| {
        let mut stmt = c.prepare("SELECT username FROM users ORDER BY username")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<String>>>()
    })
    .unwrap_or_default()
}

/// 刪帳號(連同該帳號的私密記憶 scope = username 一起清掉)。
pub fn delete_user(app: &AppHandle, username: &str) -> Result<usize, String> {
    let username = username.trim().to_string();
    let _ = db(app, |c| {
        c.execute("DELETE FROM memories WHERE scope = ?1", [&username])
    });
    db(app, |c| {
        c.execute("DELETE FROM users WHERE username = ?1", [&username])
    })
}

/* ---------------- 邀請碼(一次性,owner 產生 → 使用者自助註冊) ---------------- */

#[derive(serde::Serialize)]
pub struct Invite {
    pub code: String,
    pub used_by: Option<String>,
}

/// 產生一組新邀請碼(8 位十六進位),回傳該碼。
pub fn create_invite(app: &AppHandle) -> Result<String, String> {
    let code = random_hex(4);
    db(app, |c| {
        c.execute("INSERT INTO invite_codes (code) VALUES (?1)", [&code])
            .map(|_| ())
    })?;
    Ok(code)
}

pub fn list_invites(app: &AppHandle) -> Vec<Invite> {
    db(app, |c| {
        let mut stmt =
            c.prepare("SELECT code, used_by FROM invite_codes ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(Invite {
                code: r.get::<_, String>(0)?,
                used_by: r.get::<_, Option<String>>(1)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<Invite>>>()
    })
    .unwrap_or_default()
}

pub fn delete_invite(app: &AppHandle, code: &str) -> Result<usize, String> {
    db(app, |c| {
        c.execute("DELETE FROM invite_codes WHERE code = ?1", [code.trim()])
    })
}

/// 用邀請碼自助註冊:驗碼未使用 → 建帳號 → 標記該碼用掉。
pub fn register_with_invite(
    app: &AppHandle,
    username: &str,
    password: &str,
    code: &str,
) -> Result<(), String> {
    let code = code.trim();
    if code.is_empty() {
        return Err("請輸入邀請碼".into());
    }
    // 先確認碼存在且未被使用
    match db(app, |c| {
        c.query_row(
            "SELECT used_by FROM invite_codes WHERE code = ?1",
            [code],
            |r| r.get::<_, Option<String>>(0),
        )
    }) {
        Ok(Some(_)) => return Err("這個邀請碼已經被用過了".into()),
        Ok(None) => {} // 未使用,可繼續
        Err(_) => return Err("邀請碼無效".into()),
    }
    // 建帳號(可能因帳號已存在/保留字失敗;失敗則不消耗邀請碼)
    create_user(app, username, password)?;
    // 標記邀請碼已用
    let _ = db(app, |c| {
        c.execute(
            "UPDATE invite_codes SET used_by = ?1 WHERE code = ?2",
            rusqlite::params![username.trim(), code],
        )
    });
    Ok(())
}

/* ---------------- session token ---------------- */

/// 登入成功 → 發 token 並登記到 session 表
pub fn issue_token(app: &AppHandle, username: &str) -> String {
    let token = random_hex(32);
    if let Some(state) = app.try_state::<AuthState>() {
        state
            .0
            .lock()
            .unwrap()
            .insert(token.clone(), username.to_string());
    }
    token
}

/// token → username(無效回 None)
pub fn identity_for_token(app: &AppHandle, token: &str) -> Option<String> {
    let state = app.try_state::<AuthState>()?;
    let g = state.0.lock().unwrap();
    g.get(token.trim()).cloned()
}

/* ---------------- Tauri 命令(設定面板:owner 管帳號) ---------------- */

#[tauri::command]
pub fn auth_create_user(app: AppHandle, username: String, password: String) -> Result<(), String> {
    create_user(&app, &username, &password)
}

#[tauri::command]
pub fn auth_list_users(app: AppHandle) -> Vec<String> {
    list_users(&app)
}

#[tauri::command]
pub fn auth_delete_user(app: AppHandle, username: String) -> Result<usize, String> {
    delete_user(&app, &username)
}

#[tauri::command]
pub fn auth_create_invite(app: AppHandle) -> Result<String, String> {
    create_invite(&app)
}

#[tauri::command]
pub fn auth_list_invites(app: AppHandle) -> Vec<Invite> {
    list_invites(&app)
}

#[tauri::command]
pub fn auth_delete_invite(app: AppHandle, code: String) -> Result<usize, String> {
    delete_invite(&app, &code)
}
