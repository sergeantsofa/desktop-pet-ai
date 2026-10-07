//! remote.rs — 區域網路遠端聊天 + Live2D + 語音 + 互動,開放給同網段其他電腦/手機。
//!
//! 路由:
//! - GET  /                    內建網頁(左聊天、右 Live2D),純 fetch,不依賴 Tauri invoke
//! - GET  /api/health          健康檢查(回桌寵名字)
//! - POST /api/chat            {message, room} → provider::complete_for_remote → {reply}
//! - POST /api/tts             {text, voice?, rate?} → Edge TTS → MP3(跟桌面同款甜美聲線)
//! - GET  /lib/pixi.min.js     pixi.js(UMD,MIT,嵌在 binary;見 assets/remote/)
//! - GET  /lib/live2d.min.js   pixi-live2d-display-lipsyncpatch(UMD,MIT,嵌在 binary)
//! - GET  /vendor/<檔>         Cubism Core(專有,**不嵌入**,從 appdata vendor 即時讀)
//! - GET  /models/<路徑>       模型/characters.json(從 appdata models 即時讀,同 fileserver)
//!
//! 設計重點:
//! - 綁定 IP/埠可在設定調整(Settings.remote_host / remote_port);host 留空 = 0.0.0.0。
//! - 每個 room(每台瀏覽器一個隨機 id)各自保留滾動對話歷史(同 Discord 每頻道歷史)。
//! - 伺服器可重啟:改 host/埠後在設定按「啟動」就停舊綁新(不必重開 App)。
//! - 她在遠端回話 → emit "remote-activity",桌面端冒泡泡。
//! - Live2D + 語音 + 點擊/閒置互動:瀏覽器端用 pixi + Cubism Core 渲染、用 model.speak() 對嘴,
//!   是 stage.ts 的精簡移植版(寫在 PAGE_HTML)。聲音走 /api/tts(Edge),失敗退回瀏覽器內建語音。
//!
//! ⚠️ 安全:開到 0.0.0.0 等於同網段任何人都能跟她聊天、下控制指令(放歌/清桌面/設提醒…)。
//!    遠端工具子集已排除開網頁/讀剪貼簿/改原始碼(見 provider::remote_tool_specs),
//!    但仍請只在你信任的內網開啟。

use std::{
    collections::HashMap,
    net::UdpSocket,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::llm::SettingsState;

/// 預設埠(設定填 0 時使用)
const DEFAULT_PORT: u16 = 8765;
/// 每個 room 保留的對話則數(user + assistant 合計)
const HISTORY_KEEP: usize = 12;

/// Live2D runtime(UMD,MIT 授權,嵌在 binary)。Cubism Core 是專有授權,不嵌、改從 appdata 供應。
const PIXI_JS: &[u8] = include_bytes!("../assets/remote/pixi.min.js");
const LIVE2D_JS: &[u8] = include_bytes!("../assets/remote/live2d.min.js");
const JS_CT: &str = "text/javascript; charset=utf-8";

/// 執行中的伺服器把手:用 shutdown 旗標讓 accept 迴圈跳出 → drop server → 釋放埠
struct Running {
    shutdown: Arc<AtomicBool>,
    handle: JoinHandle<()>,
    bound: String,     // 實際綁定位址,例如 "0.0.0.0:8765"
    urls: Vec<String>, // 給別台電腦開的網址
}

/// 遠端伺服器狀態:執行把手 + appdata 根目錄(serve /models 與 /vendor 用)
pub struct RemoteState {
    running: Mutex<Option<Running>>,
    root: PathBuf,
}

impl RemoteState {
    pub fn new(root: PathBuf) -> Self {
        Self {
            running: Mutex::new(None),
            root,
        }
    }
}

/// 各 room(每台瀏覽器一個)滾動對話歷史
type Convos = Arc<Mutex<HashMap<String, Vec<(String, String)>>>>;

/* ---------------- 啟停 ---------------- */

/// 依目前設定啟動(或重啟)遠端伺服器,回傳人類可讀狀態。
pub fn try_start(app: &AppHandle) -> String {
    let settings = {
        let s = app.state::<SettingsState>();
        let g = s.0.lock().unwrap();
        g.clone()
    };
    let state = app.state::<RemoteState>();

    if !settings.remote_enabled {
        stop(state.inner());
        return "區網遠端聊天未啟用(請先在設定打開開關)。".into();
    }

    let host = {
        let h = settings.remote_host.trim();
        if h.is_empty() { "0.0.0.0".to_string() } else { h.to_string() }
    };
    let port = if settings.remote_port == 0 { DEFAULT_PORT } else { settings.remote_port };

    // 先停舊的(改 host/埠要重綁);join 確保埠已釋放再綁新的
    stop(state.inner());

    let root = state.root.clone();
    match start(app, root, &host, port) {
        Ok(running) => {
            let bound = running.bound.clone();
            let urls = running.urls.clone();
            *state.running.lock().unwrap() = Some(running);
            let where_to = if urls.is_empty() {
                "(抓不到區網 IP,請在設定填上這台電腦的 IPv4)".to_string()
            } else {
                urls.join("  或  ")
            };
            format!("遠端聊天已啟動(綁定 {bound})。其他電腦/手機用瀏覽器開:{where_to}")
        }
        Err(e) => e,
    }
}

/// 停止執行中的伺服器(若有)。join 等 accept 迴圈確實退出、埠釋放。
fn stop(state: &RemoteState) {
    let running = state.running.lock().unwrap().take();
    if let Some(r) = running {
        r.shutdown.store(true, Ordering::SeqCst);
        let _ = r.handle.join();
    }
}

/// 綁定並起一條 accept 執行緒;綁定失敗(IP 非本機網卡/埠被占)直接回錯。
fn start(app: &AppHandle, root: PathBuf, host: &str, port: u16) -> Result<Running, String> {
    let addr = format!("{host}:{port}");
    let server = Server::http(addr.as_str()).map_err(|e| {
        format!("綁定 {addr} 失敗:{e}。請確認這個 IP 是本機網卡的位址、且埠 {port} 沒被占用。")
    })?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_t = shutdown.clone();
    let app_t = app.clone();
    let root = Arc::new(root);

    let handle = thread::spawn(move || {
        let convos: Convos = Arc::new(Mutex::new(HashMap::new()));
        loop {
            if shutdown_t.load(Ordering::SeqCst) {
                break;
            }
            // 逾時輪詢:每 400ms 回頭檢查 shutdown,讓重啟/關閉能即時停下
            match server.recv_timeout(Duration::from_millis(400)) {
                Ok(Some(req)) => {
                    let app_r = app_t.clone();
                    let convos_r = convos.clone();
                    let root_r = root.clone();
                    // 慢請求(等 LLM / TTS)丟到 async runtime,不卡住 accept 迴圈
                    tauri::async_runtime::spawn(handle_request(app_r, convos_r, root_r, req));
                }
                Ok(None) => {}     // 逾時,沒有請求
                Err(_) => break,   // 伺服器壞了就收工
            }
        }
        // server 在此 drop,埠釋放
    });

    Ok(Running {
        shutdown,
        handle,
        bound: addr,
        urls: access_urls(host, port),
    })
}

/* ---------------- 請求處理 ---------------- */

async fn handle_request(app: AppHandle, convos: Convos, root: Arc<PathBuf>, mut req: Request) {
    let method = req.method().clone();
    let url = req.url().to_string();
    let path = url.split(['?', '#']).next().unwrap_or("").to_string();

    match (method, path.as_str()) {
        (Method::Options, _) => respond_cors_preflight(req),
        (Method::Get, "/") | (Method::Get, "/index.html") => respond_html(req, PAGE_HTML),
        (Method::Get, "/lib/pixi.min.js") => respond_bytes(req, PIXI_JS, JS_CT),
        (Method::Get, "/lib/live2d.min.js") => respond_bytes(req, LIVE2D_JS, JS_CT),
        (Method::Get, "/api/health") => {
            let name = {
                let s = app.state::<SettingsState>();
                let g = s.0.lock().unwrap();
                g.persona.name.clone()
            };
            respond_json(req, json!({ "ok": true, "name": name }));
        }
        // 登入:帳號密碼 → session token
        (Method::Post, "/api/login") => {
            let mut body = String::new();
            if std::io::Read::read_to_string(req.as_reader(), &mut body).is_err() {
                respond_status(req, 400, "讀取請求內容失敗");
                return;
            }
            let v: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
            let username = v["username"].as_str().unwrap_or("").trim().to_string();
            let password = v["password"].as_str().unwrap_or("").to_string();
            if crate::auth::verify_login(&app, &username, &password) {
                let token = crate::auth::issue_token(&app, &username);
                respond_json(req, json!({ "token": token, "username": username }));
            } else {
                respond_status(req, 401, "帳號或密碼錯誤");
            }
        }
        // 用邀請碼自助註冊 → 成功直接發 token(自動登入)
        (Method::Post, "/api/register") => {
            let mut body = String::new();
            if std::io::Read::read_to_string(req.as_reader(), &mut body).is_err() {
                respond_status(req, 400, "讀取請求內容失敗");
                return;
            }
            let v: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
            let username = v["username"].as_str().unwrap_or("").trim().to_string();
            let password = v["password"].as_str().unwrap_or("").to_string();
            let code = v["code"].as_str().unwrap_or("").to_string();
            match crate::auth::register_with_invite(&app, &username, &password, &code) {
                Ok(()) => {
                    let token = crate::auth::issue_token(&app, &username);
                    respond_json(req, json!({ "token": token, "username": username }));
                }
                Err(e) => respond_json(req, json!({ "error": e })),
            }
        }
        // 驗證目前 token 是否有效(網頁載入時判斷要不要顯示登入畫面)
        (Method::Get, "/api/whoami") => {
            match bearer_token(&req).and_then(|t| crate::auth::identity_for_token(&app, &t)) {
                Some(u) => respond_json(req, json!({ "username": u })),
                None => respond_status(req, 401, "未登入"),
            }
        }
        // 個人設定(per-user):每位登入帳號各自的角色大小/泡泡樣式/人設/語音/遊戲開關
        (Method::Get, "/api/usersettings") => {
            match bearer_token(&req).and_then(|t| crate::auth::identity_for_token(&app, &t)) {
                Some(u) => respond_json(req, crate::memory::user_settings(&app, &u)),
                None => respond_status(req, 401, "未登入"),
            }
        }
        (Method::Post, "/api/usersettings") => {
            let username = match bearer_token(&req)
                .and_then(|t| crate::auth::identity_for_token(&app, &t))
            {
                Some(u) => u,
                None => {
                    respond_status(req, 401, "需要登入");
                    return;
                }
            };
            let mut body = String::new();
            if std::io::Read::read_to_string(req.as_reader(), &mut body).is_err() {
                respond_status(req, 400, "讀取請求內容失敗");
                return;
            }
            match serde_json::from_str::<Value>(&body) {
                Ok(v) if v.is_object() && body.len() < 8000 => {
                    match crate::memory::set_user_settings(&app, &username, &body) {
                        Ok(()) => respond_json(req, json!({ "ok": true })),
                        Err(e) => respond_status(req, 500, &e),
                    }
                }
                _ => respond_status(req, 400, "格式錯誤(需 JSON 物件)"),
            }
        }
        // 模型檔 / Cubism Core:從 appdata 即時讀(防穿越在 fileserver::read_file)
        (Method::Get, p) if p.starts_with("/models/") || p.starts_with("/vendor/") => {
            match crate::fileserver::read_file(&root, p) {
                Some((bytes, ct)) => respond_bytes(req, &bytes, ct),
                None => respond_status(req, 404, "not found"),
            }
        }
        // 語音合成:伺服器端 Edge TTS(跟桌面同款甜美聲線),回 MP3;失敗回非 200 讓網頁退回瀏覽器語音
        (Method::Post, "/api/tts") => {
            if bearer_token(&req)
                .and_then(|t| crate::auth::identity_for_token(&app, &t))
                .is_none()
            {
                respond_status(req, 401, "需要登入");
                return;
            }
            let mut body = String::new();
            if std::io::Read::read_to_string(req.as_reader(), &mut body).is_err() {
                respond_status(req, 400, "讀取請求內容失敗");
                return;
            }
            let v: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
            let mut text = v["text"].as_str().unwrap_or("").trim().to_string();
            if text.is_empty() {
                respond_status(req, 400, "no text");
                return;
            }
            if text.chars().count() > 500 {
                text = text.chars().take(500).collect();
            }
            let voice = v["voice"]
                .as_str()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("zh-CN-XiaoyiNeural")
                .to_string();
            let rate = v["rate"].as_f64().unwrap_or(1.0) as f32;
            let rate_pct = ((rate.clamp(0.5, 2.0) - 1.0) * 100.0) as i32;
            match tauri::async_runtime::spawn_blocking(move || {
                crate::speech::run_edge(&text, &voice, rate_pct)
            })
            .await
            {
                Ok(Ok(bytes)) => respond_bytes(req, &bytes, "audio/mpeg"),
                Ok(Err(e)) => {
                    eprintln!("[remote] TTS 失敗(退回瀏覽器語音): {e}");
                    respond_status(req, 502, "tts failed");
                }
                Err(_) => respond_status(req, 500, "tts join failed"),
            }
        }
        (Method::Post, "/api/chat") => {
            // 需登入;身分=帳號(同時當對話室 + 記憶 scope)
            let identity = match bearer_token(&req)
                .and_then(|t| crate::auth::identity_for_token(&app, &t))
            {
                Some(u) => u,
                None => {
                    respond_status(req, 401, "需要登入");
                    return;
                }
            };
            let mut body = String::new();
            if std::io::Read::read_to_string(req.as_reader(), &mut body).is_err() {
                respond_status(req, 400, "讀取請求內容失敗");
                return;
            }
            let v: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
            let message = v["message"].as_str().unwrap_or("").trim().to_string();
            let room = identity.clone();
            if message.is_empty() {
                respond_json(req, json!({ "error": "訊息是空的" }));
                return;
            }

            let settings = {
                let s = app.state::<SettingsState>();
                let g = s.0.lock().unwrap();
                g.clone()
            };

            // 把這句推進該 room 歷史,取出快照(不持鎖跨 await)
            let history = {
                let mut map = convos.lock().unwrap();
                let h = map.entry(room.clone()).or_default();
                h.push(("user".to_string(), message.clone()));
                trim(h);
                h.clone()
            };

            let request_id = format!("remote-{room}");
            // Phase 1:身分先用 room(每台瀏覽器各自記憶);Phase 2 登入後換成帳號
            match crate::llm::provider::complete_for_remote(
                &app, &settings, &history, &request_id, &room,
            )
            .await
            {
                Ok(reply) => {
                    {
                        let mut map = convos.lock().unwrap();
                        let h = map.entry(room.clone()).or_default();
                        h.push(("assistant".to_string(), reply.clone()));
                        trim(h);
                    }
                    // 通知桌面端:有人在遠端找她 → 冒泡泡
                    let _ = app.emit(
                        "remote-activity",
                        json!({ "room": room, "text": message, "reply": reply }),
                    );
                    respond_json(req, json!({ "reply": reply }));
                }
                Err(e) => {
                    eprintln!("[remote] 產生回覆失敗: {e}");
                    respond_json(req, json!({ "error": e }));
                }
            }
        }
        _ => respond_status(req, 404, "not found"),
    }
}

/// 把單一 room 歷史裁到最近 HISTORY_KEEP 則
fn trim(h: &mut Vec<(String, String)>) {
    let len = h.len();
    if len > HISTORY_KEEP {
        h.drain(0..len - HISTORY_KEEP);
    }
}

/* ---------------- HTTP 回應小工具(都帶 CORS) ---------------- */

fn header(key: &str, value: &str) -> Header {
    Header::from_bytes(key.as_bytes(), value.as_bytes()).unwrap()
}

/// 從 Authorization: Bearer <token> 取出 token
fn bearer_token(req: &Request) -> Option<String> {
    req.headers().iter().find_map(|h| {
        if h.field.equiv("Authorization") {
            h.value.as_str().strip_prefix("Bearer ").map(|t| t.trim().to_string())
        } else {
            None
        }
    })
}

fn respond_json(req: Request, v: Value) {
    let resp = Response::from_string(v.to_string())
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Access-Control-Allow-Origin", "*"));
    let _ = req.respond(resp);
}

fn respond_html(req: Request, html: &str) {
    let resp = Response::from_string(html)
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
        .with_header(header("Access-Control-Allow-Origin", "*"));
    let _ = req.respond(resp);
}

fn respond_bytes(req: Request, data: &[u8], ct: &str) {
    let resp = Response::from_data(data.to_vec())
        .with_header(header("Content-Type", ct))
        .with_header(header("Access-Control-Allow-Origin", "*"));
    let _ = req.respond(resp);
}

fn respond_status(req: Request, code: u16, msg: &str) {
    let resp = Response::from_string(msg)
        .with_status_code(code)
        .with_header(header("Access-Control-Allow-Origin", "*"));
    let _ = req.respond(resp);
}

fn respond_cors_preflight(req: Request) {
    let resp = Response::from_string("")
        .with_status_code(204)
        .with_header(header("Access-Control-Allow-Origin", "*"))
        .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
        .with_header(header("Access-Control-Allow-Headers", "Content-Type, Authorization"));
    let _ = req.respond(resp);
}

/* ---------------- 區網 IP 偵測 ---------------- */

/// 取得這台電腦對外的主要 IPv4(用 UDP「假連線」騙路由表挑出網卡,不會真的送封包)。
fn primary_ipv4() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip.to_string())
    }
}

/// 組「別台電腦該開的網址」:綁 0.0.0.0 時用偵測到的區網 IP;綁特定 IP 就用該 IP。
fn access_urls(host: &str, port: u16) -> Vec<String> {
    let mut ips: Vec<String> = Vec::new();
    if host == "0.0.0.0" || host.is_empty() {
        if let Some(ip) = primary_ipv4() {
            ips.push(ip);
        }
    } else {
        ips.push(host.to_string());
    }
    ips.into_iter().map(|ip| format!("http://{ip}:{port}/")).collect()
}

/* ---------------- Tauri 命令 ---------------- */

#[derive(serde::Serialize)]
pub struct RemoteStatus {
    pub running: bool,
    pub bound: String,
    pub urls: Vec<String>,
}

/// 設定面板「啟動/重啟」按鈕呼叫:依目前設定啟停伺服器。
#[tauri::command]
pub fn remote_connect(app: AppHandle) -> String {
    try_start(&app)
}

/// 目前伺服器狀態(供 UI 顯示連結)。
#[tauri::command]
pub fn remote_status(app: AppHandle) -> RemoteStatus {
    let state = app.state::<RemoteState>();
    let g = state.running.lock().unwrap();
    match g.as_ref() {
        Some(r) => RemoteStatus {
            running: true,
            bound: r.bound.clone(),
            urls: r.urls.clone(),
        },
        None => RemoteStatus {
            running: false,
            bound: String::new(),
            urls: Vec::new(),
        },
    }
}

/// 偵測本機區網 IPv4,讓 UI 提示「這台電腦的位址大概是…」。
#[tauri::command]
pub fn remote_local_ips() -> Vec<String> {
    let mut v = Vec::new();
    if let Some(ip) = primary_ipv4() {
        v.push(ip);
    }
    v
}

/* ---------------- 內建網頁(左聊天、右 Live2D;含語音與互動) ---------------- */

const PAGE_HTML: &str = r##"<!DOCTYPE html>
<html lang="zh-Hant">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, viewport-fit=cover" />
<title>桌寵</title>
<script src="/lib/pixi.min.js"></script>
<script src="/vendor/live2dcubismcore.min.js"></script>
<script src="/lib/live2d.min.js"></script>
<style>
  :root { --pink:#ff8fb1; --bg:#fff5f8; }
  * { box-sizing:border-box; -webkit-tap-highlight-color:transparent; }
  html,body { height:100%; margin:0; }
  body { font-family:"Segoe UI","Microsoft JhengHei",sans-serif; background:var(--bg);
         color:#333; height:100dvh; overflow:hidden; user-select:none; -webkit-user-select:none; }
  #wrap { display:flex; height:100dvh; }
  /* 左:聊天 */
  #chat { width:360px; min-width:280px; display:flex; flex-direction:column; background:#fff;
          border-right:1px solid #ffd5e2; }
  header { background:var(--pink); color:#fff; padding:10px 14px; font-weight:700; font-size:17px;
           display:flex; align-items:center; justify-content:space-between; }
  .hdr-btns { display:flex; gap:8px; }
  #mute, #logout { background:rgba(255,255,255,.25); border:none; color:#fff; font-size:16px; line-height:1;
          width:34px; height:34px; border-radius:50%; cursor:pointer; }
  /* 登入畫面 */
  #login { position:fixed; inset:0; z-index:50; display:flex; align-items:center; justify-content:center;
           background:rgba(255,245,248,.96); }
  #login.hidden { display:none; }
  .login-card { background:#fff; border:1px solid #ffd5e2; border-radius:16px; padding:24px;
                width:280px; max-width:86vw; display:flex; flex-direction:column; gap:10px;
                box-shadow:0 8px 30px rgba(0,0,0,.15); }
  .login-title { font-weight:700; font-size:18px; color:var(--pink); text-align:center; }
  .login-card input { border:1px solid #ffc2d4; border-radius:10px; padding:10px 12px; font-size:16px; outline:none; }
  .login-card input:focus { border-color:var(--pink); }
  .login-card button { background:var(--pink); color:#fff; border:none; border-radius:20px;
                       padding:10px; font-size:15px; font-weight:700; cursor:pointer; }
  .login-msg { color:#c66; font-size:13px; min-height:16px; margin:0; text-align:center; }
  .login-toggle { color:var(--pink-d, #e96f95); font-size:13px; text-align:center; text-decoration:none; cursor:pointer; }
  #log { flex:1; overflow-y:auto; padding:14px; display:flex; flex-direction:column; gap:10px;
         -webkit-overflow-scrolling:touch; }
  .msg { max-width:86%; padding:9px 13px; border-radius:16px; line-height:1.5;
         white-space:pre-wrap; word-break:break-word; font-size:15px; user-select:text; -webkit-user-select:text; }
  .me  { align-self:flex-end; background:var(--pink); color:#fff; border-bottom-right-radius:4px; }
  .her { align-self:flex-start; background:#fff5f8; border:1px solid #ffd5e2; border-bottom-left-radius:4px; }
  .tip { align-self:center; color:#c79; font-size:13px; }
  footer { display:flex; gap:8px; padding:10px; border-top:1px solid #ffd5e2; padding-bottom:calc(10px + env(safe-area-inset-bottom)); }
  #box { flex:1; resize:none; border:1px solid #ffc2d4; border-radius:20px; padding:10px 14px;
         font-size:16px; outline:none; max-height:120px; font-family:inherit; }
  #box:focus { border-color:var(--pink); }
  #send { background:var(--pink); color:#fff; border:none; border-radius:20px; padding:0 18px;
          font-size:15px; font-weight:700; cursor:pointer; }
  #send:disabled { opacity:.5; cursor:default; }
  /* 右:Live2D */
  #stage { flex:1; position:relative; overflow:hidden; isolation:isolate;
           background:#fff5f8; box-shadow:inset 3px 0 16px rgba(0,0,0,.04), inset -3px 0 16px rgba(0,0,0,.04); }
  #sky, #weatherFx { position:absolute; inset:0; pointer-events:none; }
  #sky { z-index:0; transition:background 1.2s ease, filter 1.2s ease;
         background:linear-gradient(160deg,#ffeaf1,#fff5f8 60%,#eef4ff); }
  #stars { position:absolute; inset:0; pointer-events:none; z-index:0;
    opacity:0; transition:opacity 1.8s ease; }
  #stage.phase-night #stars, #stage.phase-dusk #stars { opacity:1; }
  #stage.phase-dawn #stars { opacity:.15; }
  #stars .twinkle { position:absolute; border-radius:50%;
    animation:starPulse 3s ease-in-out infinite alternate; }
  #moodOverlay { position:absolute; inset:0; pointer-events:none; z-index:1;
    transition:background 2s ease; }
  #stage.phase-dawn #moodOverlay { background:rgba(255,190,140,.12); }
  #stage.phase-morning #moodOverlay { background:rgba(255,230,180,.06); }
  #stage.phase-day #moodOverlay { background:transparent; }
  #stage.phase-sunset #moodOverlay { background:rgba(255,120,70,.16); }
  #stage.phase-dusk #moodOverlay { background:rgba(150,80,160,.10); }
  #stage.phase-night #moodOverlay { background:rgba(30,40,90,.15); }
  #glassFx { position:absolute; inset:0; pointer-events:none; z-index:2; overflow:hidden; }
  .glass-streak { position:absolute; left:var(--gx); border-radius:50%;
    background:linear-gradient(180deg,rgba(200,218,245,.55) 10%,rgba(180,200,230,.08));
    animation:glassStreak var(--gdur) ease-in forwards; animation-delay:var(--gdelay); }
  #stage::after { content:""; position:absolute; left:0; right:0; bottom:0; height:22%;
    pointer-events:none; z-index:1;
    background:linear-gradient(0deg,rgba(0,0,0,.06) 0%,rgba(0,0,0,.02) 40%,transparent 100%);
    transition:opacity 1.2s ease; }
  #sky::before, #sky::after { content:""; position:absolute; pointer-events:none; transition:opacity 1.2s ease; }
  #sky::before { width:34vmin; height:34vmin; border-radius:50%; right:10%; top:10%;
                 background:radial-gradient(circle,rgba(255,247,193,.9),rgba(255,188,119,.36) 48%,transparent 70%);
                 filter:blur(1px); opacity:.72; }
  #sky::after { left:0; right:0; bottom:0; height:38%;
                background:linear-gradient(0deg,rgba(255,255,255,.54),transparent); opacity:.7; }
  #stage.phase-dawn #sky { background:linear-gradient(165deg,#485b8f 0%,#ffb69e 48%,#fff0dc 100%); }
  #stage.phase-morning #sky { background:linear-gradient(165deg,#9ed7ff 0%,#fff5cf 55%,#ffeaf1 100%); }
  #stage.phase-day #sky { background:linear-gradient(165deg,#8fd4ff 0%,#e8f8ff 54%,#fff5f8 100%); }
  #stage.phase-sunset #sky { background:linear-gradient(165deg,#654f92 0%,#ff9870 45%,#ffd39d 100%); }
  #stage.phase-dusk #sky { background:linear-gradient(165deg,#314063 0%,#7c638f 48%,#ffc4a5 100%); }
  #stage.phase-night #sky { background:linear-gradient(165deg,#11172b 0%,#253154 55%,#4a4268 100%); }
  #stage.phase-night #sky::before { right:13%; top:12%; width:22vmin; height:22vmin;
                 background:radial-gradient(circle,rgba(245,247,255,.95),rgba(203,214,255,.28) 45%,transparent 68%); opacity:.72; }
  #stage.phase-sunset #sky::before, #stage.phase-dusk #sky::before { right:6%; top:46%; opacity:.88; }
  #weatherFx { z-index:1; overflow:hidden; }
  #weatherFx::before, #weatherFx::after { content:""; position:absolute; inset:0; pointer-events:none; opacity:0; transition:opacity 1s ease; }
  #stage.weather-clear #weatherFx::before { opacity:.34;
    background:radial-gradient(circle at 72% 22%,rgba(255,255,221,.72),transparent 20%),
               radial-gradient(circle at 28% 70%,rgba(255,255,255,.22),transparent 24%); }
  #stage.weather-cloudy #weatherFx::before { opacity:.46;
    background:radial-gradient(ellipse at 22% 24%,rgba(255,255,255,.58),transparent 22%),
               radial-gradient(ellipse at 75% 34%,rgba(255,255,255,.42),transparent 24%),
               linear-gradient(180deg,rgba(214,222,235,.28),transparent 58%); }
  #stage.weather-fog #weatherFx::before { opacity:.72; animation:fogDriftSlow 28s ease-in-out infinite alternate;
    background:radial-gradient(ellipse 120% 30% at 50% 70%,rgba(255,255,255,.52),transparent 70%); filter:blur(12px); }
  #stage.weather-fog #weatherFx::after { opacity:.48; animation:fogDriftFast 18s ease-in-out infinite alternate;
    background:radial-gradient(ellipse 100% 25% at 40% 50%,rgba(230,240,255,.44),transparent 68%); filter:blur(18px); }
  #stage.weather-storm #weatherFx::after { opacity:.55; animation:stormFlash 9s linear infinite;
    background:linear-gradient(115deg,transparent 0 52%,rgba(255,255,255,.7) 53%,transparent 55%); }
  #weatherFx .drop { position:absolute; top:-18%; left:var(--x); width:var(--w); height:var(--h);
    border-radius:999px; background:linear-gradient(180deg,rgba(180,200,230,.08),rgba(180,200,230,.58));
    opacity:var(--o); transform:rotate(14deg); animation:rainFall var(--dur) linear infinite; animation-delay:var(--delay); }
  #weatherFx .flake { position:absolute; top:-10%; left:var(--x); width:var(--s); height:var(--s);
    border-radius:50%; background:rgba(255,255,255,.9); box-shadow:0 0 8px rgba(255,255,255,.55);
    opacity:.82; animation:snowFall var(--dur) linear infinite; animation-delay:var(--delay); }
  #sceneHud { position:absolute; top:10px; right:10px; z-index:4; display:flex; gap:8px; flex-wrap:wrap; justify-content:flex-end; pointer-events:none; }
  .stage-badge { min-width:58px; padding:6px 10px; border-radius:999px; text-align:center;
    color:#3c3540; background:rgba(255,255,255,.72); border:1px solid rgba(255,255,255,.72);
    box-shadow:0 4px 16px rgba(80,55,80,.14); backdrop-filter:blur(10px); font-size:12px; font-weight:700; }
  #live2d { position:relative; z-index:2; width:100%; height:100%; display:block; touch-action:none; }
  #stagemsg { position:absolute; inset:0; display:flex; align-items:center; justify-content:center;
              text-align:center; color:#c79; font-size:14px; padding:20px; z-index:6; }
  /* 角色頭上的台詞氣泡(點擊/閒置台詞用) */
  #bubble { position:absolute; left:50%; top:14px; transform:translateX(-50%);
            max-width:80%; background:#fff; color:#444; border:1px solid #ffd5e2;
            border-radius:14px; padding:8px 12px; font-size:14px; line-height:1.4;
            box-shadow:0 4px 14px rgba(0,0,0,.12); opacity:0; transition:opacity .25s;
            pointer-events:none; white-space:pre-wrap; z-index:5; }
  #bubble.show { opacity:1; }
  /* 退場:慢慢往上飄 + 淡出 */
  #bubble.fade { opacity:0; transform:translateX(-50%) translateY(-34px);
                 transition:opacity .8s ease, transform .8s ease; }
  @keyframes rainFall { to { transform:translate3d(-20px,128vh,0) rotate(14deg); } }
  @keyframes snowFall {
    0% { transform:translate3d(0,-8vh,0); }
    50% { transform:translate3d(var(--drift),52vh,0); }
    100% { transform:translate3d(calc(var(--drift) * -0.45),112vh,0); }
  }
  @keyframes stormFlash {
    0%, 45%, 100% { opacity:0; }
    46% { opacity:.08; }
    47% { opacity:.82; }
    48% { opacity:.06; }
    49% { opacity:.6; }
    50% { opacity:0; }
    78% { opacity:0; }
    79% { opacity:.32; }
    80% { opacity:0; }
    81% { opacity:.12; }
    82% { opacity:0; }
  }
  @keyframes fogDriftSlow {
    0% { transform:translateX(-6%); }
    100% { transform:translateX(6%); }
  }
  @keyframes fogDriftFast {
    0% { transform:translateX(8%); }
    100% { transform:translateX(-8%); }
  }
  @keyframes starPulse {
    0% { opacity:.15; transform:scale(.7); }
    100% { opacity:1; transform:scale(1.2); }
  }
  @keyframes glassStreak {
    0% { top:-4%; width:5px; height:5px; opacity:0; border-radius:50%; }
    5% { opacity:.7; }
    12% { width:4px; height:8px; border-radius:40%; top:4%; }
    25% { width:3px; height:28px; border-radius:30%; top:18%; }
    50% { width:2px; height:50px; border-radius:25%; top:42%; }
    75% { width:1.5px; height:70px; border-radius:20%; top:70%; }
    100% { width:1px; height:90px; border-radius:15%; opacity:.06; top:108%; }
  }
  @media (max-width: 720px) {
    #wrap { flex-direction:column-reverse; }          /* 手機:Live2D 在上、聊天在下 */
    #chat { width:auto; height:55%; border-right:none; border-top:1px solid #ffd5e2; }
    #stage { height:45%; }
    #sceneHud { top:8px; right:8px; gap:6px; }
    .stage-badge { min-width:50px; padding:5px 8px; font-size:11px; }
    #weatherFx .drop { opacity:.52; }
  }
  /* 泡泡樣式(per-user,classic = 預設外觀) */
  #bubble.bs-cloud { border-radius:26px; }
  #bubble.bs-comic { color:#111; border:3px solid #111; box-shadow:4px 4px 0 #111; font-weight:700; }
  #bubble.bs-neon { background:rgba(18,16,38,.95); color:#7ef9ff; border:1.5px solid #7ef9ff; box-shadow:0 0 10px rgba(126,249,255,.65); }
  #bubble.bs-pastel { background:linear-gradient(135deg,#ffd9ec,#d9e8ff); color:#6a5a7a; border:none; border-radius:20px; }
  #bubble.bs-pixel { color:#222; border:3px solid #222; border-radius:0; box-shadow:4px 4px 0 rgba(0,0,0,.25); font-family:'Courier New',monospace; }
  #bubble.bs-candy { background:linear-gradient(135deg,#fff1c9,#ffd0a6); color:#8a4b2b; border:none; border-radius:22px; }
  #bubble.bs-ghost { background:rgba(255,255,255,.28); color:#fff; border:1px solid rgba(255,255,255,.6); backdrop-filter:blur(6px); text-shadow:0 1px 3px rgba(0,0,0,.55); }
  /* 我的設定面板 */
  #settings { position:fixed; inset:0; background:rgba(0,0,0,.45); display:flex; align-items:center; justify-content:center; z-index:30; }
  #settings.hidden { display:none; }
  .set-card { width:min(440px,92vw); max-height:88vh; display:flex; flex-direction:column; background:#fff; color:#333; border-radius:14px; overflow:hidden; box-shadow:0 12px 40px rgba(0,0,0,.4); }
  .set-head { display:flex; justify-content:space-between; align-items:center; padding:12px 16px; font-weight:600; border-bottom:1px solid #eee; }
  .set-head button { border:none; background:none; font-size:16px; cursor:pointer; }
  .set-body { padding:12px 16px; overflow-y:auto; }
  .set-body label { display:block; margin:0 0 12px; font-size:13px; color:#555; }
  .set-body input[type=range], .set-body select, .set-body textarea { width:100%; box-sizing:border-box; margin-top:5px; }
  .set-body select, .set-body textarea { padding:7px 8px; border:1px solid #ddd; border-radius:8px; font-size:14px; font-family:inherit; }
  .set-check { display:flex; align-items:center; gap:8px; }
  .set-check input { width:auto; margin-top:0; }
  .set-foot { display:flex; justify-content:space-between; align-items:center; padding:12px 16px; border-top:1px solid #eee; }
  #setMsg { font-size:12px; color:#4a9; }
  #setSave { border:none; background:#5b8def; color:#fff; padding:8px 20px; border-radius:16px; cursor:pointer; }
</style>
</head>
<body>
  <div id="wrap">
    <div id="chat">
      <header><span id="title">桌寵</span><span class="hdr-btns"><button id="setBtn" title="我的設定">⚙️</button><button id="logout" title="登出">⎋</button><button id="mute" title="開關語音">🔊</button></span></header>
      <div id="log"><div class="tip">跟她打個招呼吧~(點右邊的她也會有反應)</div></div>
      <footer>
        <textarea id="box" rows="1" placeholder="輸入訊息,Enter 送出(Shift+Enter 換行)…"></textarea>
        <button id="send">送出</button>
      </footer>
    </div>
    <div id="stage">
      <div id="sky"></div>
      <div id="stars" aria-hidden="true"></div>
      <div id="weatherFx" aria-hidden="true"></div>
      <div id="moodOverlay" aria-hidden="true"></div>
      <div id="glassFx" aria-hidden="true"></div>
      <div id="sceneHud" aria-live="polite">
        <div id="timeBadge" class="stage-badge">台灣 --:--</div>
        <div id="weatherBadge" class="stage-badge">晴</div>
      </div>
      <canvas id="live2d"></canvas>
      <div id="bubble"></div>
      <div id="stagemsg">載入 Live2D 中…</div>
    </div>
  </div>
  <div id="login">
    <form id="loginForm" class="login-card">
      <div class="login-title" id="loginTitle">登入</div>
      <input id="lu" type="text" placeholder="帳號" autocomplete="username" />
      <input id="lp" type="password" placeholder="密碼" autocomplete="current-password" />
      <input id="lcode" type="text" placeholder="邀請碼" style="display:none" />
      <button type="submit" id="loginBtn">登入</button>
      <p id="loginMsg" class="login-msg"></p>
      <a id="toggleReg" class="login-toggle" href="#">沒有帳號?用邀請碼註冊</a>
    </form>
  </div>
  <div id="settings" class="hidden">
    <div class="set-card">
      <div class="set-head"><span>我的設定</span><button id="setClose" title="關閉">✕</button></div>
      <div class="set-body">
        <label>角色大小:<span id="scVal">100%</span>
          <input id="scRange" type="range" min="0.5" max="2" step="0.05" value="1" />
        </label>
        <label>對話泡泡樣式
          <select id="bsSel">
            <option value="classic">經典白</option>
            <option value="cloud">雲朵</option>
            <option value="comic">漫畫框</option>
            <option value="neon">霓虹發光</option>
            <option value="pastel">粉彩漸層</option>
            <option value="pixel">像素風</option>
            <option value="candy">糖果</option>
            <option value="ghost">透明玻璃</option>
          </select>
        </label>
        <label>語音聲線
          <select id="voiceSel">
            <option value="zh-CN-XiaoyiNeural">曉伊(甜美)</option>
            <option value="zh-CN-XiaoxiaoNeural">曉曉(溫柔)</option>
            <option value="zh-CN-XiaozhenNeural">曉甄(活潑)</option>
            <option value="zh-TW-HsiaoChenNeural">曉臻(台灣)</option>
            <option value="zh-TW-HsiaoYuNeural">曉雨(台灣)</option>
            <option value="zh-CN-YunxiNeural">雲希(男聲)</option>
          </select>
        </label>
        <label>語速:<span id="rtVal">1.0x</span>
          <input id="rtRange" type="range" min="0.5" max="2" step="0.1" value="1" />
        </label>
        <label class="set-check"><input id="gkChk" type="checkbox" /> 遊戲知識庫模式(只用知識庫回答,不知道就說不知道)</label>
        <label>人設提示詞(你專屬的個性)
          <textarea id="ppTxt" rows="4" placeholder="例:你是個傲嬌但溫柔的小夥伴,講話簡短可愛…"></textarea>
        </label>
      </div>
      <div class="set-foot"><span id="setMsg"></span><button id="setSave">儲存</button></div>
    </div>
  </div>
<script>
  /* ============ 共用狀態 ============ */
  let model = null;          // Live2DModel,載入後才有
  let idleMs = 180000;       // 閒置門檻(由 characters.json 的 idleMinutes 決定)
  let muted = localStorage.getItem('petMuted') === '1';
  let talking = false;       // 瀏覽器內建語音時的正弦對嘴開關
  let curAudio = null;       // <audio> 後備播放
  // per-user 設定(從 /api/usersettings 載入;每個帳號各自一份)
  let userVoice = 'zh-CN-XiaoyiNeural', userRate = 1, userScale = 1, userBubbleStyle = 'classic';
  let doFit = null;          // initLive2D 內把 fit() 指到這裡,讓「改角色大小」能即時重新佈局
  const pick = a => a[Math.floor(Math.random() * a.length)];

  /* 桌面互動台詞(移植自 stage.ts) */
  const TAP_BODY_LINES = ["呀!幹嘛戳我啦~","嗯?找我有事嗎?","嘿嘿,我在喔。","再戳我要生氣囉!(才不會)",
    "喂喂,手放開啦,很癢欸!","戳夠了沒~人家又不是泡泡紙。","你是不是很無聊?要不要陪我聊天?",
    "哼,只有想戳我的時候才想起我喔?","摸魚被我看到囉~快回去工作啦!","再戳…我可是會記仇的喔(小聲)"];
  const TAP_HEAD_LINES = ["嗯~摸頭好舒服…","欸嘿嘿…","頭髮要亂了啦~","再多摸一下下嘛…",
    "呼…被摸頭整個人都軟了。","你的手好溫暖喔…","哼,勉強讓你摸一下啦。"];
  const IDLE_LINES = ["呼啊…有點睏了…","(東張西望)","今天過得還好嗎?","…zzZ","好安靜喔…大家都在忙嗎?",
    "無聊死了啦,陪我玩咩~","我在這裡乖乖等你喔。","剛剛那個…算了,沒事。","(偷偷看了你一眼)",
    "要不要喝口水、休息一下?","嗯…肚子有點餓了呢。"];
  const HEAD_REACT_EXPR = ["爱心眼", "星星眼"];
  const BODY_REACT_EXPR = ["脸红"];
  const HEAD_AREA = /head|face|頭/i;

  /* ============ 行動裝置音訊解鎖 ============ */
  let audioUnlocked = false;
  function unlockAudio() {
    if (audioUnlocked) return; audioUnlocked = true;
    try { const Ctx = window.AudioContext || window.webkitAudioContext; if (Ctx) { const c = new Ctx(); if (c.resume) c.resume(); } } catch (e) {}
  }
  window.addEventListener('pointerdown', unlockAudio, { passive: true });

  /* ============ 語音 ============ */
  // emoji → 她口中的情緒語氣詞(用她的聲音唸,有感情),取代把 emoji 名稱唸出來;
  // 先列到的群組先比對,沒對應的 emoji 之後一律移除。與桌面 tts.ts 的 EMOJI_SAY 同步。
  const EMOJI_SAY = [
    ["😀😃😄😁😆😅😂🤣😸😹", "哈哈"],
    ["😍🥰😘😗😙😚🤗😻❤🧡💛💚💙💜🤎🖤🤍💕💖💗💓💞💘💝💟", "嘿嘿"],
    ["😊☺🙂😌😉😋😎😏😳🤭🙈", "嘿嘿"],
    ["😜😝😛", "嘻嘻"],
    ["😢😭😞😔😟😣😖🥺😿😩😫", "嗚嗚"],
    ["😮😲😯😱🙀😨😰😧", "咦"],
    ["😡😠🤬👿💢😤", "哼"],
    ["😴😪🥱💤", "呼啊"],
    ["🤔🤨", "嗯"],
    ["👋", "嗨"],
    ["✨🌟⭐💫🎉🎊🥳", "哇"],
    ["🎵🎶", "啦啦"],
    ["😺🐱", "喵"],
  ];
  function emojiToSpeech(text) {
    let t = text;
    for (const [chars, say] of EMOJI_SAY) t = t.replace(new RegExp('[' + chars + ']', 'gu'), ' ' + say + ' ');
    return t.replace(/[\p{Extended_Pictographic}‍️\u{1F3FB}-\u{1F3FF}\u{1F1E6}-\u{1F1FF}]/gu, ' ');
  }
  function cleanSpeech(t) {
    return emojiToSpeech(t).replace(/[((][^))]*[))]/g, ' ').replace(/https?:\/\/\S+/g, ' ')
            .replace(/[~~…]+/g, '。').replace(/\s+/g, ' ').trim();
  }
  function stopSpeak() {
    try { if (model && model.stopSpeaking) model.stopSpeaking(); } catch (e) {}
    if (curAudio) { try { curAudio.pause(); } catch (e) {} curAudio = null; }
    if ('speechSynthesis' in window) window.speechSynthesis.cancel();
    talking = false;
  }
  async function speak(text) {
    const clean = cleanSpeech(text);
    if (!clean || muted) return;
    stopSpeak();
    // 先試伺服器端 Edge TTS(跟桌面同款甜美聲線)→ model.speak 真實對嘴
    try {
      const r = await fetch('/api/tts', {
        method: 'POST', headers: authHeaders(),
        body: JSON.stringify({ text: clean, voice: userVoice, rate: userRate })
      });
      if (r.ok) {
        const buf = await r.arrayBuffer();
        const url = URL.createObjectURL(new Blob([buf], { type: 'audio/mpeg' }));
        if (model && typeof model.speak === 'function') {
          model.speak(url, { volume: 1, crossOrigin: 'anonymous',
            onFinish: () => URL.revokeObjectURL(url), onError: () => URL.revokeObjectURL(url) });
          return;
        }
        curAudio = new Audio(url); curAudio.onended = () => URL.revokeObjectURL(url);
        await curAudio.play(); return;
      }
    } catch (e) {}
    // 退回瀏覽器內建語音(離線時)
    speakSystem(clean);
  }
  function speakSystem(text) {
    if (!('speechSynthesis' in window)) return;
    const u = new SpeechSynthesisUtterance(text);
    const vs = window.speechSynthesis.getVoices();
    const zh = vs.find(v => /^zh(-|_)?(TW|HK|Hant)/i.test(v.lang)) || vs.find(v => /^zh/i.test(v.lang));
    if (zh) { u.voice = zh; u.lang = zh.lang; } else { u.lang = 'zh-TW'; }
    u.onstart = () => talking = true; u.onend = () => talking = false; u.onerror = () => talking = false;
    window.speechSynthesis.speak(u);
  }

  /* ============ 聊天 ============ */
  const log = document.getElementById('log');
  const box = document.getElementById('box');
  const send = document.getElementById('send');
  const muteBtn = document.getElementById('mute');

  let room = localStorage.getItem('petRoom');
  if (!room) { room = 'web-' + Math.random().toString(36).slice(2, 10); localStorage.setItem('petRoom', room); }

  /* ============ 登入 / token ============ */
  let token = localStorage.getItem('petToken') || '';
  function authHeaders() {
    const h = { 'Content-Type': 'application/json' };
    if (token) h['Authorization'] = 'Bearer ' + token;
    return h;
  }
  const loginEl = document.getElementById('login');
  const loginForm = document.getElementById('loginForm');
  const loginMsg = document.getElementById('loginMsg');
  const lcode = document.getElementById('lcode');
  const loginTitle = document.getElementById('loginTitle');
  const loginBtn = document.getElementById('loginBtn');
  const toggleReg = document.getElementById('toggleReg');
  let regMode = false;
  function setRegMode(on) {
    regMode = on;
    lcode.style.display = on ? 'block' : 'none';
    loginTitle.textContent = on ? '註冊' : '登入';
    loginBtn.textContent = on ? '註冊' : '登入';
    toggleReg.textContent = on ? '已有帳號?改用登入' : '沒有帳號?用邀請碼註冊';
    loginMsg.textContent = '';
  }
  toggleReg.addEventListener('click', (e) => { e.preventDefault(); setRegMode(!regMode); });
  function showLogin() { loginEl.classList.remove('hidden'); const u = document.getElementById('lu'); if (u) u.focus(); }
  function hideLogin() { loginEl.classList.add('hidden'); box.focus(); loadUserSettings(); }
  function onAuthed(token0) {
    token = token0; localStorage.setItem('petToken', token);
    loginMsg.textContent = ''; document.getElementById('lp').value = ''; lcode.value = '';
    hideLogin();
  }
  loginForm.addEventListener('submit', async (e) => {
    e.preventDefault();
    unlockAudio();
    const u = document.getElementById('lu').value.trim();
    const p = document.getElementById('lp').value;
    loginMsg.textContent = regMode ? '註冊中…' : '登入中…';
    try {
      if (regMode) {
        const r = await fetch('/api/register', {
          method: 'POST', headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username: u, password: p, code: lcode.value.trim() })
        });
        const d = await r.json();
        if (d.token) onAuthed(d.token);
        else loginMsg.textContent = d.error || '註冊失敗';
      } else {
        const r = await fetch('/api/login', {
          method: 'POST', headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username: u, password: p })
        });
        if (r.ok) { const d = await r.json(); onAuthed(d.token); }
        else loginMsg.textContent = '帳號或密碼錯誤';
      }
    } catch (err) { loginMsg.textContent = '連線失敗:' + err; }
  });
  document.getElementById('logout').addEventListener('click', () => {
    token = ''; localStorage.removeItem('petToken'); showLogin();
  });
  // 開頁先驗證 token;有效就直接進,沒有/失效就顯示登入畫面(預設顯示)
  (async () => {
    if (token) {
      try { const r = await fetch('/api/whoami', { headers: authHeaders() }); if (r.ok) { hideLogin(); return; } } catch (e) {}
    }
    showLogin();
  })();

  function renderMute() { muteBtn.textContent = muted ? '🔇' : '🔊'; }
  renderMute();
  muteBtn.addEventListener('click', () => {
    muted = !muted; localStorage.setItem('petMuted', muted ? '1' : '0');
    if (muted) stopSpeak(); renderMute(); unlockAudio();
  });

  fetch('/api/health').then(r => r.json()).then(d => {
    if (d && d.name) { document.getElementById('title').textContent = d.name; document.title = d.name; }
  }).catch(() => {});

  function add(text, cls) {
    const el = document.createElement('div');
    el.className = 'msg ' + cls; el.textContent = text;
    log.appendChild(el); log.scrollTop = log.scrollHeight;
    return el;
  }

  async function submit() {
    const text = box.value.trim();
    if (!text) return;
    unlockAudio();
    box.value = '';
    add(text, 'me');
    send.disabled = true;
    const pending = add('…', 'her');
    try {
      const r = await fetch('/api/chat', {
        method: 'POST', headers: authHeaders(),
        body: JSON.stringify({ message: text })
      });
      if (r.status === 401) { pending.textContent = '(登入過期,請重新登入)'; showLogin(); return; }
      const d = await r.json();
      const reply = d.reply || ('(' + (d.error || '她沒有回應') + ')');
      pending.textContent = reply;
      if (d.reply) speak(d.reply);   // 唸出回覆 + 對嘴
    } catch (e) {
      pending.textContent = '(連線失敗:' + e + ')';
    } finally {
      send.disabled = false; box.focus();
    }
  }
  send.addEventListener('click', submit);
  box.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submit(); }
  });

  /* ============ 台詞氣泡 ============ */
  const bubbleEl = document.getElementById('bubble');
  let bubbleTimer = 0;
  function bubble(text, ms) {
    bubbleEl.textContent = text;
    // 跟著角色頭頂:把氣泡底端放在頭頂上方一點(沒有模型時退回預設 top)
    try {
      if (model) {
        const b = model.getBounds();
        const headY = b.y + b.height * 0.18;
        const sh = stage.clientHeight || window.innerHeight;
        bubbleEl.style.top = 'auto';
        bubbleEl.style.bottom = Math.max(20, Math.min(sh - 50, sh - headY + 10)) + 'px';
      }
    } catch (e) {}
    bubbleEl.classList.remove('fade');
    bubbleEl.classList.add('show');
    clearTimeout(bubbleTimer);
    bubbleTimer = setTimeout(() => {
      bubbleEl.classList.add('fade'); // 慢慢往上飄 + 淡出
      setTimeout(() => { bubbleEl.classList.remove('show'); bubbleEl.classList.remove('fade'); }, 800);
    }, ms || 4000);
  }

  /* ============ 台灣時間 + 天氣視覺(不顯示氣溫) ============ */
  const stage = document.getElementById('stage');
  const weatherFx = document.getElementById('weatherFx');
  const timeBadge = document.getElementById('timeBadge');
  const weatherBadge = document.getElementById('weatherBadge');
  const WEATHER_LABELS = { clear:'晴', cloudy:'多雲', rain:'雨', snow:'雪', storm:'雷雨', fog:'霧' };
  const WEATHER_URL = 'https://api.open-meteo.com/v1/forecast?latitude=25.033&longitude=121.565&current=weather_code&timezone=Asia%2FTaipei';
  let timePhase = 'day';
  let weatherKind = 'clear';

  function taiwanParts() {
    const parts = new Intl.DateTimeFormat('en-US', {
      timeZone:'Asia/Taipei', hour:'2-digit', minute:'2-digit', hourCycle:'h23'
    }).formatToParts(new Date());
    const map = {};
    for (const p of parts) map[p.type] = p.value;
    const hour = Number(map.hour) % 24;
    const hh = String(hour).padStart(2, '0');
    const mm = map.minute || '00';
    return { hour, minute: mm, label: `${hh}:${mm}` };
  }
  function phaseFromHour(h) {
    if (h >= 5 && h < 7) return 'dawn';
    if (h >= 7 && h < 11) return 'morning';
    if (h >= 11 && h < 17) return 'day';
    if (h >= 17 && h < 18) return 'sunset';
    if (h >= 18 && h < 20) return 'dusk';
    return 'night';
  }
  function weatherFromCode(code) {
    if (code === 0 || code === 1) return 'clear';
    if (code === 2 || code === 3) return 'cloudy';
    if (code === 45 || code === 48) return 'fog';
    if ((code >= 51 && code <= 67) || (code >= 80 && code <= 82)) return 'rain';
    if ((code >= 71 && code <= 77) || code === 85 || code === 86) return 'snow';
    if (code === 95 || code === 96 || code === 99) return 'storm';
    return 'clear';
  }
  function applyScene() {
    for (const c of Array.from(stage.classList)) {
      if (c.startsWith('phase-') || c.startsWith('weather-')) stage.classList.remove(c);
    }
    stage.classList.add('phase-' + timePhase, 'weather-' + weatherKind);
    weatherBadge.textContent = WEATHER_LABELS[weatherKind] || '晴';
  }
  function renderWeatherParticles() {
    weatherFx.innerHTML = '';
    const mobile = window.matchMedia('(max-width: 720px)').matches;
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (reduce) return;
    const rain = weatherKind === 'rain' || weatherKind === 'storm';
    const snow = weatherKind === 'snow';
    const count = rain ? (mobile ? 28 : 50) : snow ? (mobile ? 22 : 44) : 0;
    for (let i = 0; i < count; i++) {
      const el = document.createElement('span');
      el.className = rain ? 'drop' : 'flake';
      el.style.setProperty('--x', (Math.random() * 104 - 2).toFixed(2) + '%');
      el.style.setProperty('--delay', (-Math.random() * 4).toFixed(2) + 's');
      el.style.setProperty('--dur', (rain ? 0.85 + Math.random() * 0.7 : 6 + Math.random() * 6).toFixed(2) + 's');
      if (rain) {
        const fg = i < 6; // 前 6 滴為前景層
        el.style.setProperty('--w', (fg ? 2.2 + Math.random() * 1.0 : 1.2 + Math.random() * 1.2).toFixed(1) + 'px');
        el.style.setProperty('--h', (fg ? 52 + Math.random() * 28 : 32 + Math.random() * 36).toFixed(0) + 'px');
        el.style.setProperty('--o', (fg ? 0.25 + Math.random() * 0.2 : 0.35 + Math.random() * 0.35).toFixed(2));
        if (fg) el.style.filter = 'blur(1.5px)';
      }
      if (snow) {
        el.style.setProperty('--s', (3 + Math.random() * 4).toFixed(1) + 'px');
        el.style.setProperty('--drift', (Math.random() * 90 - 45).toFixed(1) + 'px');
      }
      weatherFx.appendChild(el);
    }
  }
  function generateStars() {
    const container = document.getElementById('stars');
    if (!container) return;
    container.innerHTML = '';
    const count = 80;
    for (let i = 0; i < count; i++) {
      const s = document.createElement('span');
      s.className = 'twinkle';
      const size = 0.8 + Math.random() * 2.4;
      s.style.cssText = `left:${Math.random()*100}%;top:${Math.random()*70}%;` +
        `width:${size}px;height:${size}px;background:rgba(255,255,255,${0.5+Math.random()*0.5});` +
        `animation-delay:${(Math.random()*7).toFixed(1)}s;animation-duration:${(2+Math.random()*4).toFixed(1)}s;`;
      container.appendChild(s);
    }
  }
  /* ============ 玻璃水珠(下雨時水滴附著滑落) ============ */
  let glassTimeout = null;
  function spawnGlassDrop() {
    const c = document.getElementById('glassFx');
    if (!c) return;
    const el = document.createElement('span');
    el.className = 'glass-streak';
    el.style.setProperty('--gx', (3 + Math.random() * 94).toFixed(1) + '%');
    el.style.setProperty('--gdur', (4 + Math.random() * 6).toFixed(1) + 's');
    el.style.setProperty('--gdelay', '0s');
    c.appendChild(el);
    el.addEventListener('animationend', () => el.remove());
  }
  function scheduleGlassDrop() {
    const delay = 800 + Math.random() * 2200;
    glassTimeout = setTimeout(() => { spawnGlassDrop(); scheduleGlassDrop(); }, delay);
  }
  function startGlassDrops() {
    stopGlassDrops();
    const c = document.getElementById('glassFx');
    if (!c) return;
    // 初始散落一批，錯開動畫階段
    for (let i = 0; i < 10; i++) {
      const el = document.createElement('span');
      el.className = 'glass-streak';
      el.style.setProperty('--gx', (3 + Math.random() * 94).toFixed(1) + '%');
      el.style.setProperty('--gdur', (4 + Math.random() * 6).toFixed(1) + 's');
      el.style.setProperty('--gdelay', (-Math.random() * 8).toFixed(1) + 's');
      c.appendChild(el);
      el.addEventListener('animationend', () => el.remove());
    }
    scheduleGlassDrop();
  }
  function stopGlassDrops() {
    if (glassTimeout) { clearTimeout(glassTimeout); glassTimeout = null; }
    const c = document.getElementById('glassFx');
    if (c) c.innerHTML = '';
  }
  function updateTaiwanClock() {
    const p = taiwanParts();
    const nextPhase = phaseFromHour(p.hour);
    timeBadge.textContent = '台灣 ' + p.label;
    if (nextPhase !== timePhase) {
      timePhase = nextPhase;
      applyScene();
    }
  }
  async function refreshWeather() {
    try {
      const r = await fetch(WEATHER_URL, { cache:'no-store' });
      if (!r.ok) throw new Error('weather ' + r.status);
      const d = await r.json();
      const code = Number(d && d.current && d.current.weather_code);
      weatherKind = Number.isFinite(code) ? weatherFromCode(code) : 'clear';
    } catch (e) {
      weatherKind = 'clear';
    }
    applyScene();
    renderWeatherParticles();
    if (weatherKind === 'rain' || weatherKind === 'storm') {
      startGlassDrops();
    } else {
      stopGlassDrops();
    }
  }
  updateTaiwanClock();
  applyScene();
  renderWeatherParticles();
  refreshWeather();
  generateStars();
  stopGlassDrops();
  setInterval(updateTaiwanClock, 60000);
  setInterval(refreshWeather, 15 * 60000);
  window.addEventListener('resize', renderWeatherParticles);

  /* ============ Live2D ============ */
  const canvas = document.getElementById('live2d');
  const stageMsg = document.getElementById('stagemsg');
  function setStageMsg(t) { if (t) { stageMsg.textContent = t; stageMsg.style.display = 'flex'; } else { stageMsg.style.display = 'none'; } }
  function tryExpr(name) { try { if (model) model.expression(name); } catch (e) {} }
  function randomExpr() {
    try {
      const mgr = model.internalModel.motionManager.expressionManager;
      if (mgr && mgr.definitions && mgr.definitions.length) model.expression(Math.floor(Math.random() * mgr.definitions.length));
    } catch (e) {}
  }
  function playMotion(prefer) {
    try {
      const defs = (model.internalModel.motionManager.definitions) || {};
      for (const g of prefer) { if (defs[g] && defs[g].length) { model.motion(g); return; } }
    } catch (e) {}
  }
  let gazeYOffset = 0; // 視線垂直偏移:基準從模型中心上移到頭部中心(fit() 重算)
  function lookAround() {
    try {
      const r = canvas.getBoundingClientRect();
      model.focus(r.left + r.width * (0.25 + Math.random() * 0.5), r.top + r.height * (0.2 + Math.random() * 0.45) + gazeYOffset);
    } catch (e) {}
  }

  let lastInteract = Date.now();
  let lastHover = 0;
  function markI() { lastInteract = Date.now(); }

  async function initLive2D() {
    try {
      if (!window.PIXI || !window.PIXI.live2d) { setStageMsg('Live2D 函式庫載入失敗'); return; }
      if (!window.Live2DCubismCore) { setStageMsg('缺 Cubism Core(伺服器端 appdata\\vendor 沒有 live2dcubismcore.min.js)'); return; }

      const app = new PIXI.Application({
        view: canvas, backgroundAlpha: 0, resizeTo: stage,
        antialias: true, autoDensity: true, resolution: window.devicePixelRatio || 1
      });

      let charPath = null, emotions = {}, fixedParams = {}, scale = 1;
      try {
        const m = await (await fetch('/models/characters.json')).json();
        const chars = m.characters || [];
        const active = chars.find(c => c.id === m.active) || chars[0];
        if (active) {
          charPath = '/models/' + String(active.path).replace(/^[\/\\]+/, '');
          emotions = active.emotions || {};
          fixedParams = active.fixedParams || {};
          scale = active.scale || 1;
          if (active.idleMinutes) idleMs = active.idleMinutes * 60000;
        }
      } catch (e) { setStageMsg('讀不到 characters.json'); return; }
      if (!charPath) { setStageMsg('characters.json 沒有角色'); return; }

      const Live2DModel = PIXI.live2d.Live2DModel;
      if (Live2DModel.registerTicker) Live2DModel.registerTicker(PIXI.Ticker);
      model = await Live2DModel.from(charPath, { autoInteract: false });
      app.stage.addChild(model);
      setStageMsg('');

      function fit() {
        const w = app.renderer.width / app.renderer.resolution;
        const h = app.renderer.height / app.renderer.resolution;
        const s = (h / model.internalModel.height) * 0.92 * scale * userScale;
        model.scale.set(s); model.anchor.set(0.5, 1); model.position.set(w / 2, h);
        // 視線基準上移到頭部中心(避免游標在頭旁邊時一直往上看)
        try { gazeYOffset = model.getBounds().height * (0.5 - 0.18); } catch (e) {}
      }
      fit();
      doFit = fit; // 給「我的設定」改角色大小時重新佈局用
      window.addEventListener('resize', fit);

      // 每幀(在模型 update 之後)固定多餘部件參數 + 系統語音時的正弦對嘴
      PIXI.Ticker.shared.add(() => {
        try {
          const core = model.internalModel.coreModel;
          for (const id in fixedParams) core.setParameterValueById(id, fixedParams[id]);
          if (talking) {
            const t = performance.now() / 1000;
            const v = Math.max(0, Math.abs(Math.sin(t * 9)) * 0.6 + Math.sin(t * 23) * 0.25);
            core.setParameterValueById('ParamMouthOpenY', Math.min(1, v));
          }
        } catch (e) {}
      });

      // 互動:點頭/點身 → 表情 + 台詞(氣泡 + 唸出);視線追蹤;懸停頭部換表情
      stage.addEventListener('pointermove', e => {
        try { const r = canvas.getBoundingClientRect(); model.focus(e.clientX - r.left, (e.clientY - r.top) + gazeYOffset); } catch (_) {}
        const now = Date.now();
        if (now - lastHover > 5000) {
          try {
            const r = canvas.getBoundingClientRect();
            if ((model.hitTest(e.clientX - r.left, e.clientY - r.top) || []).some(a => HEAD_AREA.test(a))) { lastHover = now; randomExpr(); }
          } catch (_) {}
        }
      });
      stage.addEventListener('pointerdown', e => {
        unlockAudio(); markI();
        let areas = [];
        try { const r = canvas.getBoundingClientRect(); areas = model.hitTest(e.clientX - r.left, e.clientY - r.top) || []; } catch (_) {}
        if (areas.some(a => HEAD_AREA.test(a))) {
          tryExpr(pick(HEAD_REACT_EXPR)); playMotion(['Tap', 'TapHead', 'MeiYan']);
          const line = pick(TAP_HEAD_LINES); bubble(line); speak(line);
        } else {
          tryExpr(pick(BODY_REACT_EXPR));
          const line = pick(TAP_BODY_LINES); bubble(line); speak(line);
        }
      });

      // 待機循環:有些模型閒置組叫 DaiJi 不叫 Idle,定期播以保持律動
      playMotion(['Idle', 'DaiJi']);
      setInterval(() => playMotion(['Idle', 'DaiJi']), 11000);

      // 閒置碎念 / 東張西望(跟桌面一樣)
      setInterval(() => {
        if (Date.now() - lastInteract < idleMs) return;
        markI();
        if (Math.random() < 0.5) lookAround(); else playMotion(['Idle', 'DaiJi']);
        if (Math.random() < 0.5) { const line = pick(IDLE_LINES); bubble(line); speak(line); }
      }, 30000);
    } catch (e) {
      setStageMsg('Live2D 載入失敗:' + (e && e.message ? e.message : e));
    }
  }
  /* ============ 我的設定(per-user) ============ */
  const BS_LIST = ['classic','cloud','comic','neon','pastel','pixel','candy','ghost'];
  function applyBubbleStyle() {
    const be = document.getElementById('bubble'); if (!be) return;
    for (const b of BS_LIST) be.classList.remove('bs-' + b);
    if (userBubbleStyle && userBubbleStyle !== 'classic') be.classList.add('bs-' + userBubbleStyle);
  }
  function applyUserSettings(s) {
    s = s || {};
    userScale = +s.charScale || 1;
    userBubbleStyle = s.bubbleStyle || 'classic';
    userVoice = s.ttsVoice || 'zh-CN-XiaoyiNeural';
    userRate = +s.ttsRate || 1;
    if (doFit) { try { doFit(); } catch (e) {} }
    applyBubbleStyle();
    const g = id => document.getElementById(id);
    g('scRange').value = userScale; g('scVal').textContent = Math.round(userScale*100)+'%';
    g('bsSel').value = userBubbleStyle; g('voiceSel').value = userVoice;
    g('rtRange').value = userRate; g('rtVal').textContent = (+userRate).toFixed(1)+'x';
    g('gkChk').checked = !!s.gameKb; g('ppTxt').value = s.personaPrompt || '';
  }
  async function loadUserSettings() {
    try { const r = await fetch('/api/usersettings', { headers: authHeaders() }); if (r.ok) applyUserSettings(await r.json()); } catch (e) {}
  }
  (function initSettingsUI(){
    const g = id => document.getElementById(id);
    g('scRange').addEventListener('input', () => { userScale = +g('scRange').value; g('scVal').textContent = Math.round(userScale*100)+'%'; if (doFit) { try { doFit(); } catch (e) {} } });
    g('bsSel').addEventListener('change', () => { userBubbleStyle = g('bsSel').value; applyBubbleStyle(); });
    g('rtRange').addEventListener('input', () => { userRate = +g('rtRange').value; g('rtVal').textContent = (+userRate).toFixed(1)+'x'; });
    g('voiceSel').addEventListener('change', () => { userVoice = g('voiceSel').value; });
    g('setBtn').addEventListener('click', () => g('settings').classList.remove('hidden'));
    g('setClose').addEventListener('click', () => g('settings').classList.add('hidden'));
    g('setSave').addEventListener('click', async () => {
      const payload = { charScale:+g('scRange').value, bubbleStyle:g('bsSel').value, ttsVoice:g('voiceSel').value, ttsRate:+g('rtRange').value, gameKb:g('gkChk').checked, personaPrompt:g('ppTxt').value.trim() };
      g('setMsg').textContent = '儲存中…';
      try {
        const r = await fetch('/api/usersettings', { method:'POST', headers: authHeaders(), body: JSON.stringify(payload) });
        if (r.ok) { applyUserSettings(payload); g('setMsg').textContent='已儲存!'; setTimeout(()=>g('setMsg').textContent='',1500); }
        else g('setMsg').textContent='儲存失敗';
      } catch (e) { g('setMsg').textContent='儲存失敗'; }
    });
  })();

  initLive2D();
  box.focus();
</script>
</body>
</html>
"##;
