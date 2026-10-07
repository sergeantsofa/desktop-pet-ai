//! Discord 串接:用 serenity 連 Discord,在「指定頻道」收到訊息 → 交給她現有的大腦 → 回覆。
//!
//! - Bot Token 存 Windows 認證管理員(keys.rs,provider_id = "discord"),不落明文。
//! - 只在 Settings.discord_channels 列出的頻道回應;忽略所有 bot(含她自己),避免互嗆迴圈。
//! - 回覆走 provider::complete_for_discord(人設 + 長期記憶,非串流)。
//! - serenity 的 Client 無法乾淨地熱重啟,所以改 Token/頻道後要重啟 App 才生效。

use std::sync::atomic::{AtomicBool, Ordering};

use serenity::all::*;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::llm::{keys, SettingsState};

/// 避免重複啟動多個 client(同一 process 內只連一次)
static STARTED: AtomicBool = AtomicBool::new(false);

/// 每個頻道保留的對話則數(user + assistant 合計)
const HISTORY_KEEP: usize = 12;

struct Handler {
    app: AppHandle,
    channels: Vec<u64>,
    /// 各頻道的滾動對話歷史:channel_id -> [(role, content), ...]
    convos: std::sync::Mutex<std::collections::HashMap<u64, Vec<(String, String)>>>,
}

#[serenity::async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        // 忽略所有機器人(含她自己),否則會無限互相回覆
        if msg.author.bot {
            return;
        }
        // 只回應指定頻道
        if !self.channels.contains(&msg.channel_id.get()) {
            return;
        }
        let text = msg.content.trim().to_string();
        if text.is_empty() {
            return;
        }

        // 取設定快照(人設/路由可能被使用者改過)
        let settings = {
            let s = self.app.state::<SettingsState>();
            let g = s.0.lock().unwrap();
            g.clone()
        };

        // 顯示「輸入中…」讓對方知道她在想
        let _ = msg.channel_id.broadcast_typing(&ctx.http).await;

        let author = msg.author.global_name.clone().unwrap_or_else(|| msg.author.name.clone());
        let cid = msg.channel_id.get();

        // 把這句推進該頻道歷史,並取出快照(不持鎖跨 await)
        let history = {
            let mut map = self.convos.lock().unwrap();
            let h = map.entry(cid).or_default();
            h.push(("user".to_string(), format!("{author}:{text}")));
            trim(h);
            h.clone()
        };

        match crate::llm::provider::complete_for_discord(&self.app, &settings, &history).await {
            Ok(reply) => {
                // 把她的回覆也存回歷史
                {
                    let mut map = self.convos.lock().unwrap();
                    let h = map.entry(cid).or_default();
                    h.push(("assistant".to_string(), reply.clone()));
                    trim(h);
                }
                // Discord 單則訊息上限 2000 字,保險截在 1900
                let to_send: String = if reply.chars().count() > 1900 {
                    reply.chars().take(1900).collect()
                } else {
                    reply.clone()
                };
                if let Err(e) = msg.channel_id.say(&ctx.http, &to_send).await {
                    eprintln!("[discord] 傳送失敗: {e}");
                }
                // 通知桌面端:她在 Discord 有動靜(前端冒泡泡)
                let _ = self.app.emit(
                    "discord-activity",
                    json!({ "author": author, "text": text, "reply": reply }),
                );
            }
            Err(e) => eprintln!("[discord] 產生回覆失敗: {e}"),
        }
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        println!("[discord] 已連線,身分:{}", ready.user.name);
    }
}

/// 把單一頻道歷史裁到最近 HISTORY_KEEP 則
fn trim(h: &mut Vec<(String, String)>) {
    let len = h.len();
    if len > HISTORY_KEEP {
        h.drain(0..len - HISTORY_KEEP);
    }
}

/// 依目前設定嘗試啟動 Discord bot,回傳人類可讀狀態。
/// 條件不足(未啟用 / 無 Token / 無有效頻道)時直接回報,不啟動。
pub fn try_start(app: &AppHandle) -> String {
    let settings = {
        let s = app.state::<SettingsState>();
        let g = s.0.lock().unwrap();
        g.clone()
    };
    if !settings.discord_enabled {
        return "Discord 未啟用(請先在設定打開開關)。".into();
    }
    let token = match keys::get_key("discord") {
        Some(t) if !t.trim().is_empty() => t,
        _ => return "尚未設定 Discord Bot Token。".into(),
    };
    let channels: Vec<u64> = settings
        .discord_channels
        .iter()
        .filter_map(|c| c.trim().parse::<u64>().ok())
        .collect();
    if channels.is_empty() {
        return "尚未設定任何有效的頻道 ID(要純數字)。".into();
    }
    if STARTED.swap(true, Ordering::SeqCst) {
        return "Discord 已在連線中。若剛改了 Token 或頻道,請重新啟動 App 才會生效。".into();
    }

    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT
            | GatewayIntents::DIRECT_MESSAGES;
        let handler = Handler {
            app: app2,
            channels,
            convos: std::sync::Mutex::new(std::collections::HashMap::new()),
        };
        match Client::builder(&token, intents).event_handler(handler).await {
            Ok(mut client) => {
                if let Err(e) = client.start().await {
                    eprintln!("[discord] 連線中斷: {e}");
                    STARTED.store(false, Ordering::SeqCst);
                }
            }
            Err(e) => {
                eprintln!("[discord] 建立 client 失敗(Token 可能錯誤): {e}");
                STARTED.store(false, Ordering::SeqCst);
            }
        }
    });
    "Discord 連線中…去看看你的 Bot 是不是在伺服器上線了(綠燈)。".into()
}

/// Tauri 命令:設定面板「連線」按鈕呼叫。
#[tauri::command]
pub fn discord_connect(app: AppHandle) -> String {
    try_start(&app)
}
