//! 一鍵設定:首次啟動時從官方來源下載 Cubism Core + 一個免費範例角色,
//! 放進使用者的外部資料夾,達成「開箱即用」。
//!
//! 授權:Cubism Core 由 Live2D 官方 CDN 下載(使用者同意其授權);範例角色
//! 為 Live2D 官方 sample(Haru),受 Free Material License。下載是使用者端行為,
//! 本程式不重新散布這些受保護資產(故不入版控、不打包進安裝包)。

use std::path::Path;
use tauri::{AppHandle, Emitter, Manager};

const CORE_URL: &str = "https://cubism.live2d.com/sdk-web/cubismcore/live2dcubismcore.min.js";
const MODEL_BASE: &str =
    "https://cdn.jsdelivr.net/gh/guansss/pixi-live2d-display@master/test/assets";

/// Haru 範例模型需要下載的檔案(相對 models 資料夾)
const HARU_FILES: &[&str] = &[
    "haru/haru_greeter_t03.model3.json",
    "haru/haru_greeter_t03.moc3",
    "haru/haru_greeter_t03.physics3.json",
    "haru/haru_greeter_t03.pose3.json",
    "haru/haru_greeter_t03.2048/texture_00.png",
    "haru/haru_greeter_t03.2048/texture_01.png",
    "haru/expressions/F01.exp3.json",
    "haru/expressions/F02.exp3.json",
    "haru/expressions/F03.exp3.json",
    "haru/expressions/F04.exp3.json",
    "haru/expressions/F05.exp3.json",
    "haru/expressions/F06.exp3.json",
    "haru/expressions/F07.exp3.json",
    "haru/expressions/F08.exp3.json",
    "haru/motion/haru_g_idle.motion3.json",
    "haru/motion/haru_g_m07.motion3.json",
    "haru/motion/haru_g_m15.motion3.json",
    "haru/motion/haru_g_m14.motion3.json",
    "haru/motion/haru_g_m05.motion3.json",
];

const CHARACTERS_JSON: &str = r#"{
  "active": "haru",
  "characters": [
    {
      "id": "haru",
      "name": "Haru(範例)",
      "path": "haru/haru_greeter_t03.model3.json",
      "scale": 1.0,
      "idleMinutes": 3,
      "emotions": { "happy": "motion:Tap", "surprised": "motion:Tap" }
    }
  ]
}
"#;

/// 是否已備妥可用角色(Core + 至少一個 characters.json)
#[tauri::command]
pub fn assets_ready(app: AppHandle) -> bool {
    let Ok(dir) = app.path().app_data_dir() else {
        return false;
    };
    dir.join("vendor").join("live2dcubismcore.min.js").is_file()
        && dir.join("models").join("characters.json").is_file()
}

/// 下載 Core + 範例角色到外部資料夾;進度走 "bootstrap-progress" 事件。
#[tauri::command]
pub async fn bootstrap_assets(app: AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let vendor = dir.join("vendor");
    let models = dir.join("models");
    std::fs::create_dir_all(&vendor).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&models).map_err(|e| e.to_string())?;

    let client = reqwest::Client::builder()
        .user_agent("desktop-pet-ai")
        .build()
        .map_err(|e| e.to_string())?;

    let total = HARU_FILES.len() + 1; // +1 = Core
    let mut done = 0usize;

    emit(&app, done, total, "下載 Cubism Core…");
    download(&client, CORE_URL, &vendor.join("live2dcubismcore.min.js")).await?;
    done += 1;

    for rel in HARU_FILES {
        emit(&app, done, total, "下載範例角色…");
        let url = format!("{MODEL_BASE}/{rel}");
        download(&client, &url, &models.join(rel)).await?;
        done += 1;
    }

    std::fs::write(models.join("characters.json"), CHARACTERS_JSON).map_err(|e| e.to_string())?;
    emit(&app, total, total, "完成!");
    Ok(())
}

fn emit(app: &AppHandle, done: usize, total: usize, label: &str) {
    let _ = app.emit(
        "bootstrap-progress",
        serde_json::json!({ "done": done, "total": total, "label": label }),
    );
}

async fn download(client: &reqwest::Client, url: &str, dest: &Path) -> Result<(), String> {
    let resp = client.get(url).send().await.map_err(|e| format!("連線失敗:{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下載 {url} 失敗:HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(dest, &bytes).map_err(|e| format!("寫入失敗:{e}"))
}
