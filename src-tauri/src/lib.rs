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
mod auth;
mod backup;
mod bootstrap;
mod desktop;
mod discord;
mod fileserver;
mod fish;
mod llm;
mod memory;
mod remote;
mod scheduler;
mod selfdev;
mod speech;
mod spotify;
mod vision;
mod watcher;
mod window;

/// 用系統瀏覽器開啟外部網址(「關於 / 更新 → 查看所有版本」用)。
/// 走 Rust 端的 opener 擴充,不必為此加前端 capability。
#[tauri::command]
fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    // 只允許 http/https,避免被餵 file:// 之類的本機路徑
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("只允許開啟 http/https 網址".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(window::shortcut_handler)
                .build(),
        )
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            app.manage(llm::SettingsState(Mutex::new(llm::load_settings(
                app.handle(),
            ))));
            app.manage(llm::CancelState(Mutex::new(HashSet::new())));
            app.manage(agent::PermissionState(Mutex::new(HashMap::new())));
            app.manage(spotify::SpotifyState(Mutex::new(None)));
            app.manage(desktop::HegemonyState(Mutex::new(Vec::new())));
            app.manage(auth::AuthState::default());
            app.manage(fish::FishState::default());
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
                // 啟動時若主人的人格摘要缺漏/過期,背景重算一次(失敗靜默)
                tauri::async_runtime::spawn(llm::provider::maybe_refresh_persona(
                    app.handle().clone(),
                    "owner".to_string(),
                ));
            }
            // 本機資源伺服器:serve %APPDATA%\com.desktoppet.ai\(模型/Core 外部載入)
            // 同一個 appdata 根目錄也給「區網遠端聊天」用來 serve /models 與 /vendor。
            let res_port = match app.path().app_data_dir() {
                Ok(dir) => {
                    let _ = std::fs::create_dir_all(dir.join("models"));
                    let _ = std::fs::create_dir_all(dir.join("vendor"));
                    app.manage(remote::RemoteState::new(dir.clone()));
                    fileserver::start(dir).unwrap_or(0)
                }
                Err(_) => {
                    app.manage(remote::RemoteState::new(std::path::PathBuf::new()));
                    0
                }
            };
            app.manage(fileserver::ResourcePort(res_port));

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
            window::open_settings_if_requested(app.handle());

            // Discord 串接:若已啟用且有 Token/頻道,啟動時自動連線
            println!("[discord] {}", discord::try_start(app.handle()));

            // 區網遠端聊天:若已啟用,啟動時自動綁定伺服器
            println!("[remote] {}", remote::try_start(app.handle()));

            // Fish Speech:若開了自動啟動,把本地 API server 跑起來
            println!("[fish] {}", fish::try_start(app.handle()));

            // 啟動保險:每日自動備份一次(安裝程式曾把整個 appdata 清掉)
            backup::auto_backup_on_startup(app.handle());
            Ok(())
        })
        .on_window_event(window::handle_window_event)
        .invoke_handler(tauri::generate_handler![
            llm::get_settings,
            llm::save_settings,
            llm::set_api_key,
            llm::has_api_key,
            llm::health_check,
            llm::list_models,
            llm::chat_stream,
            llm::cancel_chat,
            llm::vision_chat,
            llm::vision_chat_file,
            speech::speech_status,
            speech::setup::setup_speech,
            speech::tts_synthesize,
            speech::tts_edge,
            speech::tts_fish,
            speech::stt_transcribe,
            agent::agent_permission_response,
            selfdev::dev_list_checkpoints,
            selfdev::dev_restore_checkpoint,
            memory::load_recent_history,
            memory::clear_memories,
            memory::clear_history,
            memory::backfill_embeddings,
            memory::gamekb_add,
            memory::gamekb_list,
            memory::gamekb_delete,
            llm::provider::refresh_persona_summary,
            llm::provider::get_persona_summary,
            auth::auth_create_user,
            auth::auth_list_users,
            auth::auth_delete_user,
            auth::auth_create_invite,
            auth::auth_list_invites,
            auth::auth_delete_invite,
            window::open_data_folder,
            open_external_url,
            backup::backup_appdata,
            backup::list_backups,
            fileserver::resource_port,
            bootstrap::assets_ready,
            bootstrap::bootstrap_assets,
            discord::discord_connect,
            spotify::spotify_connect,
            spotify::spotify_playback_state,
            remote::remote_connect,
            remote::remote_status,
            remote::remote_local_ips,
            fish::fish_start,
            fish::fish_stop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
