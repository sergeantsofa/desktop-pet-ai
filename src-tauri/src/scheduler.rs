//! M4.5:主動行為排程(規格 §6 scheduler)。
//!
//! 目前職責:每 20 秒檢查到期提醒,emit "reminder-due" 給前端
//! (前端負責顯示視窗、泡泡與朗讀)。App 關閉期間錯過的提醒,
//! 啟動後第一輪檢查會補發。

use serde_json::json;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const TICK_SECS: u64 = 20;

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            check_due(&app);
            tokio::time::sleep(Duration::from_secs(TICK_SECS)).await;
        }
    });
}

fn check_due(app: &AppHandle) {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    for content in crate::memory::take_due_reminders(app, &now) {
        let _ = app.emit("reminder-due", json!({ "content": content }));
    }
}
