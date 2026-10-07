//! 一鍵設定:首次啟動時從官方來源下載 Cubism Core + 一個免費範例角色,
//! 放進使用者的外部資料夾,達成「開箱即用」。
//!
//! 授權:Cubism Core 由 Live2D 官方 CDN 下載(使用者同意其授權);範例角色
//! 為 Live2D 官方 sample(Haru),受 Free Material License。下載是使用者端行為,
//! 本程式不重新散布這些受保護資產(故不入版控、不打包進安裝包)。

use std::path::Path;
use tauri::{AppHandle, Emitter, Manager};

// Cubism Core:固定用 Cubism 4 版本(npm live2dcubismcore@1.0.0,unpkg 託管,(C) 2019 Live2D)。
// ⚠️ 不可用官方 `cubism.live2d.com/sdk-web/cubismcore/live2dcubismcore.min.js`——那是 latest,
// 現已升到 Cubism 5/Core 6.x(支援 MocVersion_50),與本專案的 pixi-live2d-display 0.5.0 不相容:
// 模型能載入但 doDrawModel 會丟 "Cannot read properties of undefined (reading '0')",角色畫不出來。
const CORE_URL: &str = "https://unpkg.com/live2dcubismcore@1.0.0/live2dcubismcore.min.js";
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

    // 合併而非覆蓋:一鍵下載只能「補上 Haru」,不能洗掉使用者既有角色
    //(舊版直接覆寫 characters.json,會把 icegirl 等角色清成只剩 Haru)。
    let path = models.join("characters.json");
    let existing = std::fs::read_to_string(&path).ok();
    let merged = merge_characters(existing.as_deref(), CHARACTERS_JSON);
    std::fs::write(&path, merged).map_err(|e| e.to_string())?;
    emit(&app, total, total, "完成!");
    Ok(())
}

/// 把 `additions`(Haru 範例)合併進 `existing` 的角色清單:
/// - 保留既有的 active 與所有角色設定(含 emotions/fixedParams/scale)
/// - 只補上 id 還不存在的角色,不覆蓋同名角色
/// - 既有檔案壞掉或不存在時,退回 `additions`
fn merge_characters(existing: Option<&str>, additions: &str) -> String {
    let Some(existing) = existing else {
        return additions.to_string();
    };
    let (Ok(mut base), Ok(add)) = (
        serde_json::from_str::<serde_json::Value>(existing),
        serde_json::from_str::<serde_json::Value>(additions),
    ) else {
        return additions.to_string();
    };

    let add_list = add["characters"].as_array().cloned().unwrap_or_default();
    let base_list = base["characters"].as_array().cloned().unwrap_or_default();

    let has_id = |list: &[serde_json::Value], id: &str| {
        list.iter()
            .any(|c| c.get("id").and_then(|v| v.as_str()) == Some(id))
    };

    let mut merged = base_list;
    for cand in add_list {
        let id = cand.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        if !id.is_empty() && !has_id(&merged, id) {
            merged.push(cand);
        }
    }

    // 沒有角色(或原本的 active 不存在)時才採用範例的 active
    let active_ok = base
        .get("active")
        .and_then(|v| v.as_str())
        .map(|a| has_id(&merged, a))
        .unwrap_or(false);
    if !active_ok {
        base["active"] = add["active"].clone();
    }
    base["characters"] = serde_json::Value::Array(merged);

    serde_json::to_string_pretty(&base).unwrap_or_else(|_| additions.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 既有角色清單(模擬使用者已調好的 appdata characters.json)
    const EXISTING: &str = r#"{
      "active": "icegirl",
      "characters": [
        { "id": "icegirl", "name": "冰女", "path": "icegirl/IceGirl.model3.json",
          "scale": 1.0, "fixedParams": { "Param59": 0 } }
      ]
    }"#;

    #[test]
    fn keeps_existing_characters_and_active() {
        let merged = merge_characters(Some(EXISTING), CHARACTERS_JSON);
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let ids: Vec<&str> = v["characters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        // 原角色保留 + Haru 被補上
        assert_eq!(ids, vec!["icegirl", "haru"]);
        // active 沒有被洗成 haru
        assert_eq!(v["active"], "icegirl");
        // 原角色的自訂設定沒有掉
        assert_eq!(v["characters"][0]["fixedParams"]["Param59"], 0);
    }

    #[test]
    fn falls_back_to_additions_without_existing_file() {
        let merged = merge_characters(None, CHARACTERS_JSON);
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["active"], "haru");
        assert_eq!(v["characters"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn recovers_from_corrupt_existing_file() {
        let merged = merge_characters(Some("{ not json"), CHARACTERS_JSON);
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["active"], "haru");
    }

    #[test]
    fn does_not_duplicate_haru() {
        let once = merge_characters(Some(EXISTING), CHARACTERS_JSON);
        let twice = merge_characters(Some(&once), CHARACTERS_JSON);
        let v: serde_json::Value = serde_json::from_str(&twice).unwrap();
        assert_eq!(v["characters"].as_array().unwrap().len(), 2);
    }
}
