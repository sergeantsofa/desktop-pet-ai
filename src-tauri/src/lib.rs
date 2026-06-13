//! Desktop Pet AI — Rust 核心(M0 骨架)
//!
//! 模組規劃(對應規格書 §6):
//! - window.rs   視窗/托盤/全域快捷鍵/位置記憶/點擊穿透 ← M0 已實作
//! - llm/        Provider 抽象、串流、降級、金鑰        ← M1 已實作
//! - speech/     STT/TTS sidecar 管理                   ← M2 已實作
//! - agent/      工具呼叫、權限確認                     ← M3 前半已實作(唯讀工具)
//! - memory/     SQLite 長期記憶 + 對話紀錄             ← M4 已實作(向量檢索留 M4.5)
//! - sandbox.rs  程式執行沙箱                           ← M3 後半
//! - scheduler.rs 主動行為排程                          ← M4.5

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};
use tauri::Manager;

mod agent;
mod llm;
mod memory;
mod scheduler;
mod selfdev;
mod speech;
mod vision;
mod watcher;
mod window;

pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(window::shortcut_handler)
                .build(),
        )
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(llm::SettingsState(Mutex::new(llm::load_settings(
                app.handle(),
            ))));
            app.manage(llm::CancelState(Mutex::new(HashSet::new())));
            app.manage(agent::PermissionState(Mutex::new(HashMap::new())));
            // 記憶資料庫開不起來不致命:工具與紀錄會回報「未就緒」
            let memory_ready = match memory::init(app.handle()) {
                Ok(db) => {
                    app.manage(db);
                    true
                }
                Err(e) => {
                    eprintln!("[memory] SQLite 初始化失敗: {e}");
                    false
                }
            };
            // 提醒排程器(需要記憶資料庫)
            if memory_ready {
                scheduler::start(app.handle().clone());
            }
            // 截圖資料夾監看(依設定;啟動時若已開啟就掛上)
            app.manage(watcher::WatchState(Mutex::new(None)));
            {
                let s = app.state::<llm::SettingsState>();
                let settings = s.0.lock().unwrap().clone();
                if settings.watch_screenshots {
                    let watch = app.state::<watcher::WatchState>();
                    if let Err(e) = watcher::apply(
                        app.handle(),
                        &watch,
                        true,
                        &settings.screenshot_dir,
                    ) {
                        eprintln!("[watcher] 啟動截圖監看失敗: {e}");
                    }
                }
            }
            window::setup(app)?;
            Ok(())
        })
        .on_window_event(window::handle_window_event)
        .invoke_handler(tauri::generate_handler![
            llm::get_settings,
            llm::save_settings,
            llm::set_api_key,
            llm::has_api_key,
            llm::health_check,
            llm::chat_stream,
            llm::cancel_chat,
            llm::vision_chat,
            llm::vision_chat_file,
            speech::speech_status,
            speech::tts_synthesize,
            speech::tts_edge,
            speech::stt_transcribe,
            agent::agent_permission_response,
            memory::load_recent_history,
            memory::clear_memories,
            memory::clear_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
