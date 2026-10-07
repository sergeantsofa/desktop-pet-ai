//! 視窗管理:透明置頂視窗、系統托盤、全域快捷鍵、位置記憶、點擊穿透。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Window, WindowEvent,
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

/// 全域快捷鍵:Ctrl+Shift+D 切換「連續對話」模式
fn converse_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyD)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct WindowGeom {
    x: i32,
    y: i32,
    // 視窗尺寸(inner，物理像素)。舊存檔只有 x/y,serde default 補 0 → 視為「沒存尺寸」。
    #[serde(default)]
    w: u32,
    #[serde(default)]
    h: u32,
}

fn pos_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(POS_FILE))
}

fn save_geometry(app: &AppHandle) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    // 視窗最小化時(含 Win+D / Win+M 顯示桌面)Windows 會把位置回報成 -32000,
    // 千萬別把這種螢幕外座標存下來,否則下次啟動視窗會還原到看不見的地方。
    if win.is_minimized().unwrap_or(false) {
        return;
    }
    let Ok(pos) = win.outer_position() else {
        return;
    };
    // 保險:座標明顯在所有螢幕之外(例如螢幕被拔掉)也不存。
    if !position_visible(&win, pos.x, pos.y) {
        return;
    }
    // 尺寸抓不到就存 0(還原時視為「沒尺寸」沿用預設)。
    let (w, h) = win
        .inner_size()
        .map(|s| (s.width, s.height))
        .unwrap_or((0, 0));
    let Some(path) = pos_path(app) else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let geom = WindowGeom {
        x: pos.x,
        y: pos.y,
        w,
        h,
    };
    if let Ok(json) = serde_json::to_string(&geom) {
        let _ = fs::write(&path, json);
    }
}

fn restore_geometry(app: &AppHandle) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let Some(path) = pos_path(app) else { return };
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(geom) = serde_json::from_str::<WindowGeom>(&text) else {
        return;
    };
    // 先還原尺寸(只在合理範圍內;舊存檔沒尺寸 = 0 就跳過,沿用 tauri.conf 預設)。
    if (200..=4000).contains(&geom.w) && (200..=4000).contains(&geom.h) {
        let _ = win.set_size(PhysicalSize::new(geom.w, geom.h));
    }
    // 只在座標落在某個螢幕的可見範圍內才還原,避免上次被最小化(Windows 記成
    // -32000)或螢幕被拔掉後,視窗還原到看不見的地方。看不見就用預設位置。
    if position_visible(&win, geom.x, geom.y) {
        let _ = win.set_position(PhysicalPosition::new(geom.x, geom.y));
    }
}

/// 視窗左上角座標 (x, y) 是否落在任一螢幕的可見範圍內(留 100px 邊界容忍,
/// 確保至少一角看得見)。拿不到螢幕資訊時保守回 false(改用預設位置)。
fn position_visible(win: &tauri::WebviewWindow, x: i32, y: i32) -> bool {
    const MARGIN: i32 = 100;
    let Ok(monitors) = win.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let mp = m.position();
        let ms = m.size();
        x >= mp.x - MARGIN
            && x <= mp.x + ms.width as i32 - MARGIN
            && y >= mp.y - MARGIN
            && y <= mp.y + ms.height as i32 - MARGIN
    })
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
    } else if shortcut.matches(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyD) {
        if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
            let _ = win.show();
        }
        let _ = app.emit("toggle-converse", ());
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
    if let Err(e) = app.global_shortcut().register(converse_shortcut()) {
        eprintln!("[window] 連續對話快捷鍵註冊失敗: {e}");
    }

    restore_geometry(app.handle());
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
                open_settings_window(app);
            }
            "quit" => {
                save_geometry(app);
                crate::fish::stop(app); // 關掉桌寵幫你啟動的 Fish(若有)
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

/// 開啟(或聚焦)獨立的「設定」視窗。它是真正的 OS 視窗(有標題列、可縮放、
/// 不透明),跟角色視窗(main)完全獨立——設定裡調的「視窗大小」只動 main,不影響本視窗。
/// 前端 main.ts 會依視窗 label = "settings" 載入設定 UI。
fn open_settings_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("設定 — Desktop Pet AI")
    .inner_size(620.0, 720.0)
    .min_inner_size(460.0, 520.0)
    .resizable(true)
    .center()
    .build();
}

/// 開發/驗證用:設 `DESKTOP_PET_OPEN_SETTINGS=1` 時,啟動就直接開設定視窗,
/// 免手動點托盤。正常使用不設這個變數就完全沒影響。
pub fn open_settings_if_requested(app: &AppHandle) {
    if std::env::var("DESKTOP_PET_OPEN_SETTINGS").is_ok() {
        open_settings_window(app);
    }
}

/// 視窗事件:關閉請求 → 隱藏到托盤(真正退出走托盤「結束」);順手保存位置
pub fn handle_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            save_geometry(window.app_handle());
            api.prevent_close();
            let _ = window.hide();
        }
        WindowEvent::Moved(_) => {
            // 拖曳結束沒有獨立事件,Moved 觸發頻繁但寫入量極小,直接保存
            save_geometry(window.app_handle());
        }
        WindowEvent::Resized(_) => {
            // 拖曳縮放把手 / 設定面板改尺寸 → 連同位置一起保存(最小化時 save_geometry 會略過)
            save_geometry(window.app_handle());
        }
        _ => {}
    }
}
