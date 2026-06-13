//! 螢幕截圖(M5:看截圖)。
//!
//! 截圖前先把桌寵主視窗藏起來,避免她把自己也拍進去;截完還原。
//! 截圖會縮到寬度上限再轉 PNG/base64,降低視覺模型的處理負擔(更快)。

use base64::Engine as _;
use image::{codecs::png::PngEncoder, imageops::FilterType, ExtendedColorType, ImageEncoder};
use std::{thread::sleep, time::Duration};
use tauri::{AppHandle, Manager};
use xcap::Monitor;

/// 縮圖寬度上限(視覺模型不需要原生 4K,縮小可大幅加速)
const MAX_WIDTH: u32 = 1366;
const MAIN_WINDOW: &str = "main";

/// 擷取主螢幕,回傳 PNG 的 base64(不含 data: 前綴)。
pub fn capture_screen_base64(app: &AppHandle) -> Result<String, String> {
    // 截圖前藏起桌寵,避免拍到自己;用 was_visible 決定要不要還原
    let win = app.get_webview_window(MAIN_WINDOW);
    let was_visible = win.as_ref().and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if was_visible {
        if let Some(w) = &win {
            let _ = w.hide();
        }
        // 等合成器把視窗移除這一幀
        sleep(Duration::from_millis(180));
    }

    let result = grab_primary();

    if was_visible {
        if let Some(w) = &win {
            let _ = w.show();
        }
    }

    rgba_to_base64(result?)
}

/// 讀取指定圖片檔,回傳 PNG base64(縮到寬度上限)。
/// 截圖剛產生時可能還在寫入,讀失敗會短暫重試。
pub fn read_image_base64(path: &str) -> Result<String, String> {
    let mut last_err = String::new();
    for attempt in 0..5 {
        if attempt > 0 {
            sleep(Duration::from_millis(300));
        }
        match image::open(path) {
            Ok(img) => return rgba_to_base64(img.to_rgba8()),
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(format!("讀取圖片失敗:{last_err}"))
}

/// RgbaImage → 縮圖 → PNG → base64
fn rgba_to_base64(rgba: image::RgbaImage) -> Result<String, String> {
    let (w, h) = (rgba.width(), rgba.height());
    let resized = if w > MAX_WIDTH {
        let nh = (h as f32 * MAX_WIDTH as f32 / w as f32).round() as u32;
        image::imageops::resize(&rgba, MAX_WIDTH, nh.max(1), FilterType::Triangle)
    } else {
        rgba
    };

    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(
            resized.as_raw(),
            resized.width(),
            resized.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG 編碼失敗:{e}"))?;

    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

fn grab_primary() -> Result<image::RgbaImage, String> {
    let monitors = Monitor::all().map_err(|e| format!("讀取螢幕失敗:{e}"))?;
    if monitors.is_empty() {
        return Err("找不到任何螢幕".into());
    }
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .unwrap_or(&monitors[0]);
    monitor.capture_image().map_err(|e| format!("截圖失敗:{e}"))
}
