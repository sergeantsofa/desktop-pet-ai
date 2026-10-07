//! LLM 模組:Provider 抽象層(OpenAI Chat Completions 相容)、設定持久化、Tauri 命令。
//!
//! - 內建 Provider:Ollama(http://localhost:11434/v1,免金鑰)、DeepSeek(https://api.deepseek.com)
//! - 任務路由:chat / coder / reasoner 各自指定 provider+model
//! - 雲端失敗自動降級本地(尚未輸出任何 token 時)
//! - API Key 僅存 Windows Credential Manager(keys.rs),settings.json 不落明文

pub mod keys;
pub mod provider;

use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::Mutex,
};
use tauri::{AppHandle, Manager, State};

pub struct SettingsState(pub Mutex<Settings>);

/// 已要求取消的 requestId 集合;串流迴圈每個 chunk 檢查一次
pub struct CancelState(pub Mutex<HashSet<String>>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCfg {
    pub id: String,
    pub name: String,
    /// OpenAI 相容端點根路徑,如 http://localhost:11434/v1
    pub base_url: String,
    /// 是否需要 API Key(從 Credential Manager 讀取,key 名稱 = id)
    pub uses_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRoute {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub name: String,
    pub system_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub providers: Vec<ProviderCfg>,
    /// 任務路由:"chat" | "coder" | "reasoner"
    pub routing: HashMap<String, TaskRoute>,
    /// 雲端失敗時的本地降級路由
    pub fallback: TaskRoute,
    pub fallback_to_local: bool,
    pub persona: Persona,
    /// 對話保留輪數(短期記憶視窗)
    pub context_turns: usize,
    /// M3:允許模型使用工具(查時間/剪貼簿/開網頁/系統狀態)
    pub agent_enabled: bool,
    /// M5.5:監看截圖資料夾,有新圖就自動看
    pub watch_screenshots: bool,
    /// 監看的資料夾(空字串 = 預設 Pictures\Screenshots)
    pub screenshot_dir: String,
    /// 層次二:允許她讀/改自己的原始碼(高風險,預設關)
    pub self_dev_enabled: bool,
    /// 自我修改的專案根目錄(空字串 = 停用)
    pub self_dev_root: String,
    /// P0 編譯閘:她改完自己的碼後自動驗證(前端型別/Rust 編譯/JSON),沒過自動還原。預設開。
    /// 做多檔重構(單檔中間狀態還不能編譯)時可關。
    pub self_dev_auto_verify: bool,
    /// Discord 串接:總開關(預設關)。Token 另存認證管理員(provider_id = "discord")
    pub discord_enabled: bool,
    /// 她會在這些頻道(純數字頻道 ID)回應每一則訊息
    pub discord_channels: Vec<String>,
    /// Spotify 串接:總開關(預設關)。refresh token 另存認證管理員(provider_id = "spotify")
    pub spotify_enabled: bool,
    /// Spotify 應用程式的 Client ID(非機密,可存設定檔)
    pub spotify_client_id: String,
    /// 區網遠端聊天:總開關(預設關)。開啟後同網段其他電腦可用瀏覽器跟她聊天/下指令。
    pub remote_enabled: bool,
    /// 綁定的 IPv4(空字串 = 0.0.0.0 全部網卡);要限定某張網卡就填該 IP。
    pub remote_host: String,
    /// 綁定的埠(0 = 預設 8765)
    pub remote_port: u16,
    /// 語意記憶用的 Ollama embedding 模型(需先 `ollama pull`);空字串 = 停用語意檢索(退回關鍵字)。
    pub embed_model: String,
    /// Fish Speech TTS:桌寵啟動時自動把本地 Fish API server 跑起來(預設關)
    pub fish_autostart: bool,
    /// Fish 啟動指令(整行,如 `python -m tools.api_server --listen 127.0.0.1:8080`);空 = 不啟動
    pub fish_launch_cmd: String,
    /// 執行 Fish 指令的工作目錄(fish-speech 專案根;空 = 繼承桌寵的工作目錄)
    pub fish_cwd: String,
    /// Fish API 根位址,用來判斷是否已在執行(避免重複啟動)。預設 http://127.0.0.1:8080
    pub fish_api_base: String,
    /// 遊戲知識庫模式:開啟後她只用「教過的遊戲知識」回答,答案不在庫裡就說不知道(不亂編)。
    pub game_kb_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let mut routing = HashMap::new();
        routing.insert(
            "chat".into(),
            TaskRoute { provider: "ollama".into(), model: "qwen2.5:7b".into() },
        );
        routing.insert(
            "coder".into(),
            TaskRoute { provider: "ollama".into(), model: "qwen2.5-coder:7b".into() },
        );
        routing.insert(
            "reasoner".into(),
            TaskRoute { provider: "ollama".into(), model: "deepseek-r1:7b".into() },
        );
        routing.insert(
            "vision".into(),
            TaskRoute { provider: "ollama".into(), model: "qwen2.5vl:3b".into() },
        );
        routing.insert(
            "gamekb".into(),
            TaskRoute { provider: "deepseek".into(), model: "deepseek-v4-flash".into() },
        );
        Self {
            providers: vec![
                ProviderCfg {
                    id: "ollama".into(),
                    name: "Ollama(本地)".into(),
                    base_url: "http://localhost:11434/v1".into(),
                    uses_key: false,
                },
                ProviderCfg {
                    id: "ollama-remote".into(),
                    name: "Ollama(遠端 192.168.60.77)".into(),
                    base_url: "http://192.168.60.77:11434/v1".into(),
                    uses_key: false,
                },
                ProviderCfg {
                    id: "deepseek".into(),
                    name: "DeepSeek(雲端)".into(),
                    base_url: "https://api.deepseek.com".into(),
                    uses_key: true,
                },
            ],
            routing,
            fallback: TaskRoute { provider: "ollama".into(), model: "qwen2.5:7b".into() },
            fallback_to_local: true,
            persona: Persona {
                name: "小桌寵".into(),
                system_prompt:
                    "你是住在使用者桌面上的小夥伴,活潑可愛、偶爾有點傲嬌。用繁體中文,口語、簡短地回答,像朋友聊天。"
                        .into(),
            },
            context_turns: 10,
            agent_enabled: true,
            watch_screenshots: false,
            screenshot_dir: String::new(),
            self_dev_enabled: false,
            self_dev_root: String::new(),
            self_dev_auto_verify: true,
            discord_enabled: false,
            discord_channels: Vec::new(),
            spotify_enabled: false,
            spotify_client_id: String::new(),
            remote_enabled: false,
            remote_host: String::new(),
            remote_port: 8765,
            embed_model: "nomic-embed-text".into(),
            fish_autostart: false,
            fish_launch_cmd: String::new(),
            fish_cwd: String::new(),
            fish_api_base: "http://127.0.0.1:8080".into(),
            game_kb_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

pub fn load_settings(app: &AppHandle) -> Settings {
    settings_path(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app).ok_or("無法取得設定目錄")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

/* ---------------- Tauri 命令 ---------------- */

#[tauri::command]
pub fn get_settings(state: State<SettingsState>) -> Settings {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<SettingsState>,
    watch: State<crate::watcher::WatchState>,
    settings: Settings,
) -> Result<(), String> {
    *state.0.lock().unwrap() = settings.clone();
    write_settings(&app, &settings)?;
    // 截圖監看設定可能變了 → 重新套用(失敗只記錄,不擋存檔)
    if let Err(e) =
        crate::watcher::apply(&app, &watch, settings.watch_screenshots, &settings.screenshot_dir)
    {
        eprintln!("[watcher] 套用截圖監看失敗: {e}");
    }
    Ok(())
}

#[tauri::command]
pub fn set_api_key(provider_id: String, key: String) -> Result<(), String> {
    keys::set_key(&provider_id, &key)
}

#[tauri::command]
pub fn has_api_key(provider_id: String) -> bool {
    keys::has_key(&provider_id)
}

/// 健康檢查:對各 provider 的 /models 發 GET(3 秒逾時)
#[tauri::command]
pub async fn health_check(
    state: State<'_, SettingsState>,
) -> Result<HashMap<String, bool>, String> {
    let providers = state.0.lock().unwrap().providers.clone();
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let mut result = HashMap::new();
    for p in providers {
        let url = format!("{}/models", p.base_url.trim_end_matches('/'));
        let mut req = client.get(&url);
        if p.uses_key {
            match keys::get_key(&p.id) {
                Some(k) => req = req.bearer_auth(k),
                None => {
                    result.insert(p.id, false);
                    continue;
                }
            }
        }
        let ok = matches!(req.send().await, Ok(r) if r.status().is_success());
        result.insert(p.id, ok);
    }
    Ok(result)
}

/// 列出某 provider 可用的模型(打它的 /models;Ollama 回已安裝、DeepSeek 回雲端清單)。
/// 任何失敗都回空陣列(前端會退回手動輸入),不擋設定面板。
#[tauri::command]
pub async fn list_models(
    state: State<'_, SettingsState>,
    provider_id: String,
) -> Result<Vec<String>, String> {
    let provider = state
        .0
        .lock()
        .unwrap()
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned();
    let Some(p) = provider else {
        return Ok(vec![]);
    };
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/models", p.base_url.trim_end_matches('/'));
    let mut req = client.get(&url);
    if p.uses_key {
        match keys::get_key(&p.id) {
            Some(k) => req = req.bearer_auth(k),
            None => return Ok(vec![]),
        }
    }
    let resp = match req.send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Ok(vec![]),
    };
    let v: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Ok(vec![]),
    };
    let mut models: Vec<String> = v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    Ok(models)
}

/// 串流對話:結果透過事件回傳
/// chat-delta {requestId, delta} / chat-done {requestId, content}
/// chat-error {requestId, message} / chat-fallback {requestId, from, to}
#[tauri::command]
pub async fn chat_stream(
    app: AppHandle,
    state: State<'_, SettingsState>,
    cancel: State<'_, CancelState>,
    request_id: String,
    task: String,
    messages: Vec<ChatMessage>,
    persist: Option<bool>,
) -> Result<(), String> {
    let settings = state.0.lock().unwrap().clone();
    // 清掉可能殘留的同 id 取消旗標
    cancel.0.lock().unwrap().remove(&request_id);
    // 主動關心等合成指令傳 persist=false,不落對話紀錄
    provider::run_chat(app, settings, request_id, task, messages, persist.unwrap_or(true)).await
}

/// 取消進行中的串流;對應請求會以當下累積內容收尾(發 chat-done)
#[tauri::command]
pub fn cancel_chat(cancel: State<CancelState>, request_id: String) {
    cancel.0.lock().unwrap().insert(request_id);
}

/// 看截圖(M5):截取主螢幕 → 送視覺模型;結果走同一組 chat-* 事件回傳。
#[tauri::command]
pub async fn vision_chat(
    app: AppHandle,
    state: State<'_, SettingsState>,
    cancel: State<'_, CancelState>,
    request_id: String,
    prompt: String,
) -> Result<(), String> {
    let settings = state.0.lock().unwrap().clone();
    cancel.0.lock().unwrap().remove(&request_id);
    let prompt = if prompt.trim().is_empty() {
        "看看我現在的螢幕,簡短說說你看到什麼、給點評論或吐槽。".to_string()
    } else {
        prompt
    };
    // 截圖是阻塞操作(藏視窗+抓圖),丟到 blocking 執行緒
    let app2 = app.clone();
    let image_b64 = tauri::async_runtime::spawn_blocking(move || {
        crate::vision::capture_screen_base64(&app2)
    })
    .await
    .map_err(|e| e.to_string())??;
    provider::run_vision(app, settings, request_id, image_b64, prompt).await
}

/// 看指定圖片檔(M5.5:截圖資料夾監看偵測到新圖時呼叫)。
#[tauri::command]
pub async fn vision_chat_file(
    app: AppHandle,
    state: State<'_, SettingsState>,
    cancel: State<'_, CancelState>,
    request_id: String,
    path: String,
    prompt: String,
) -> Result<(), String> {
    let settings = state.0.lock().unwrap().clone();
    cancel.0.lock().unwrap().remove(&request_id);
    let prompt = if prompt.trim().is_empty() {
        "這是使用者剛剛的截圖,簡短說說你看到什麼、給點評論或吐槽。".to_string()
    } else {
        prompt
    };
    let image_b64 = tauri::async_runtime::spawn_blocking(move || crate::vision::read_image_base64(&path))
        .await
        .map_err(|e| e.to_string())??;
    provider::run_vision(app, settings, request_id, image_b64, prompt).await
}
