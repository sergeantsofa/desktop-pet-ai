//! M5.5:截圖資料夾監看。
//!
//! 監看某資料夾(預設 %USERPROFILE%\Pictures\Screenshots),
//! 有新圖片出現就 emit "screenshot-added" {path};前端據此呼叫視覺模型讀圖。
//! 由設定開關(watch_screenshots / screenshot_dir)控制,可隨時開關。

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

/// 持有目前的 watcher;drop 掉(設成 None)即停止監看
pub struct WatchState(pub Mutex<Option<RecommendedWatcher>>);

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];

/// 預設截圖資料夾:%USERPROFILE%\Pictures\Screenshots
pub fn default_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    Path::new(&home).join("Pictures").join("Screenshots")
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// 套用監看設定:enabled=false 停止;enabled=true(重新)監看 dir。
/// 回傳實際監看的資料夾路徑(供 UI 顯示)。
pub fn apply(app: &AppHandle, state: &WatchState, enabled: bool, dir: &str) -> Result<String, String> {
    // 先停掉舊的(drop 掉 watcher)
    *state.0.lock().unwrap() = None;
    if !enabled {
        return Ok(String::new());
    }

    let folder = if dir.trim().is_empty() {
        default_dir()
    } else {
        PathBuf::from(dir.trim())
    };
    // 資料夾不存在就建立(Win+PrtScn 用過才會自動建)
    if !folder.exists() {
        std::fs::create_dir_all(&folder)
            .map_err(|e| format!("無法建立截圖資料夾 {}:{e}", folder.display()))?;
    }

    let app = app.clone();
    let recent: Arc<Mutex<HashMap<PathBuf, Instant>>> = Arc::new(Mutex::new(HashMap::new()));

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return;
        }
        for path in event.paths {
            if !is_image(&path) {
                continue;
            }
            // 同一檔案 5 秒內只處理一次(寫入會觸發多個事件)
            let now = Instant::now();
            {
                let mut map = recent.lock().unwrap();
                if map.get(&path).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(5)) {
                    continue;
                }
                map.insert(path.clone(), now);
                // 順手清掉太舊的紀錄
                map.retain(|_, t| now.duration_since(*t) < Duration::from_secs(30));
            }
            let _ = app.emit("screenshot-added", json!({ "path": path.to_string_lossy() }));
        }
    })
    .map_err(|e| format!("建立檔案監看失敗:{e}"))?;

    watcher
        .watch(&folder, RecursiveMode::NonRecursive)
        .map_err(|e| format!("監看 {} 失敗:{e}", folder.display()))?;

    *state.0.lock().unwrap() = Some(watcher);
    Ok(folder.display().to_string())
}
