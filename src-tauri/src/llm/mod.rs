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
        Self {
            providers: vec![
                ProviderCfg {
                    id: "ollama".into(),
                    name: "Ollama(本地)".into(),
                    base_url: "http://localhost:11434/v1".into(),
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
    settings: Settings,
) -> Result<(), String> {
    *state.0.lock().unwrap() = settings.clone();
    write_settings(&app, &settings)
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
