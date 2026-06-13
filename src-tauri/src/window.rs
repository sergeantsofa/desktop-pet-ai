//! 視窗管理:透明置頂視窗、系統托盤、全域快捷鍵、位置記憶、點擊穿透。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Emitter, Manager, PhysicalPosition, Window, WindowEvent,
};
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState,
};

const MAIN_WINDOW: &str = "main";

/// 開啟(並建立)外部資源資料夾,讓使用者放模型 / Cubism Core。回傳路徑。
#[tauri::command]
pub fn open_data_folder(app: AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let _ = fs::create_dir_all(dir.join("models"));
    let _ = fs::create_dir_all(dir.join("vendor"));
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    }
    Ok(dir.display().to_string())
}
const POS_FILE: &str = "window-position.json";

/// 全域快捷鍵:Ctrl+Shift+A 喚出/收合對話輸入框
fn chat_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyA)
}

/// 全域快捷鍵:Ctrl+Shift+S 開始/結束語音輸入(M2)
fn voice_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS)
}

/// 全域快捷鍵:Ctrl+Shift+V 讓她看一眼螢幕(M5)
fn vision_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyV)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct WindowPos {
    x: i32,
    y: i32,
}

fn pos_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(POS_FILE))
}

fn save_position(app: &AppHandle) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let Ok(pos) = win.outer_position() else {
        return;
    };
    let Some(path) = pos_path(app) else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(&WindowPos { x: pos.x, y: pos.y }) {
        let _ = fs::write(&path, json);
    }
}

fn restore_position(app: &AppHandle) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let Some(path) = pos_path(app) else { return };
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    if let Ok(pos) = serde_json::from_str::<WindowPos>(&text) {
        let _ = win.set_position(PhysicalPosition::new(pos.x, pos.y));
    }
}

/// 全域快捷鍵 handler:通知前端切換對話輸入框,並確保視窗可見、取得焦點
pub fn shortcut_handler(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    if shortcut.matches(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyA) {
        if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
            let _ = win.show();
            let _ = win.set_focus();
        }
        let _ = app.emit("toggle-chat", ());
    } else if shortcut.matches(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyS) {
        // 語音輸入不需要鍵盤焦點,只確保視窗可見(泡泡顯示「聆聽中」)
        if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
            let _ = win.show();
        }
        let _ = app.emit("toggle-voice", ());
    } else if shortcut.matches(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyV) {
        if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
            let _ = win.show();
        }
        let _ = app.emit("see-screen", ());
    }
}

pub fn setup(app: &mut App) -> tauri::Result<()> {
    // 註冊全域快捷鍵(被其他程式占用時不致命,僅記錄)
    if let Err(e) = app.global_shortcut().register(chat_shortcut()) {
        eprintln!("[window] 對話快捷鍵註冊失敗: {e}");
    }
    if let Err(e) = app.global_shortcut().register(voice_shortcut()) {
        eprintln!("[window] 語音快捷鍵註冊失敗: {e}");
    }
    if let Err(e) = app.global_shortcut().register(vision_shortcut()) {
        eprintln!("[window] 看螢幕快捷鍵註冊失敗: {e}");
    }

    restore_position(app.handle());
    build_tray(app)?;
    Ok(())
}

fn build_tray(app: &mut App) -> tauri::Result<()> {
    let toggle_visible = MenuItem::with_id(app, "toggle_visible", "顯示 / 隱藏", true, None::<&str>)?;
    let click_through =
        CheckMenuItem::with_id(app, "click_through", "點擊穿透(整個視窗)", true, false, None::<&str>)?;
    let smart_passthrough = CheckMenuItem::with_id(
        app,
        "smart_passthrough",
        "智慧穿透(角色以外區域)",
        true,
        false,
        None::<&str>,
    )?;
    let mute = CheckMenuItem::with_id(app, "mute", "靜音", true, false, None::<&str>)?;
    let see_screen = MenuItem::with_id(app, "see_screen", "看看我的螢幕", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let settings = MenuItem::with_id(app, "settings", "設定…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "結束", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &toggle_visible,
            &click_through,
            &smart_passthrough,
            &mute,
            &see_screen,
            &separator,
            &settings,
            &quit,
        ],
    )?;

    // CheckMenuItem 點擊後會自動切換勾選狀態,closure 內讀取最新狀態即可
    let click_through_item = click_through.clone();
    let smart_passthrough_item = smart_passthrough.clone();
    let mute_item = mute.clone();

    TrayIconBuilder::with_id("main-tray")
        .icon(
            app.default_window_icon()
                .expect("tauri.conf.json 缺少 icon 設定")
                .clone(),
        )
        .tooltip("Desktop Pet AI")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "toggle_visible" => {
                if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
                    if win.is_visible().unwrap_or(false) {
                        let _ = win.hide();
                    } else {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
            }
            "click_through" => {
                let enabled = click_through_item.is_checked().unwrap_or(false);
                if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = win.set_ignore_cursor_events(enabled);
                }
                // 通知前端:整窗穿透優先,智慧穿透輪詢須暫停
                let _ = app.emit("click-through-manual", enabled);
            }
            "smart_passthrough" => {
                let enabled = smart_passthrough_item.is_checked().unwrap_or(false);
                // 命中測試在前端(Live2D hitTest),由前端輪詢游標並切換穿透
                let _ = app.emit("smart-passthrough", enabled);
            }
            "mute" => {
                let muted = mute_item.is_checked().unwrap_or(false);
                let _ = app.emit("set-mute", muted);
            }
            "see_screen" => {
                if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = win.show();
                }
                let _ = app.emit("see-screen", ());
            }
            "settings" => {
                // M0:設定頁尚未實作,先通知前端顯示提示
                let _ = app.emit("open-settings", ());
            }
            "quit" => {
                save_position(app);
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

/// 視窗事件:關閉請求 → 隱藏到托盤(真正退出走托盤「結束」);順手保存位置
pub fn handle_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            save_position(window.app_handle());
            api.prevent_close();
            let _ = window.hide();
        }
        WindowEvent::Moved(_) => {
            // 拖曳結束沒有獨立事件,Moved 觸發頻繁但寫入量極小,直接保存
            save_position(window.app_handle());
        }
        _ => {}
    }
}
