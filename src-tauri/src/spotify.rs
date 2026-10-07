//! Spotify 串接:用 Web API 控制播放(播放指定歌曲 / 暫停 / 上下一首 / 音量)。
//!
//! - OAuth 2.0 Authorization Code + PKCE(只需 Client ID,不需 client secret)
//! - refresh token 存 Windows 認證管理員(keys.rs,provider_id = "spotify")
//! - access token 快取在記憶體(SpotifyState),過期自動用 refresh token 刷新
//! - ⚠️ 播放控制需 Spotify Premium + 一台正在執行的 Spotify 裝置
//! - 由 agent 的 spotify_* 工具呼叫;授權流程由設定面板的 spotify_connect 命令觸發

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::llm::{keys, SettingsState};

const REDIRECT: &str = "http://127.0.0.1:8888/callback";
const SCOPES: &str = "user-read-playback-state user-modify-playback-state";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const API: &str = "https://api.spotify.com/v1";

/// 記憶體中的存取權杖快取:(access_token, 到期時間)
pub struct SpotifyState(pub Mutex<Option<(String, Instant)>>);

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn client_id(app: &AppHandle) -> String {
    let s = app.state::<SettingsState>();
    let g = s.0.lock().unwrap();
    g.spotify_client_id.trim().to_string()
}

fn truncate(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/* ---------------- OAuth(PKCE) ---------------- */

/// 設定面板「連結 Spotify」按鈕呼叫:開瀏覽器授權 → 換 token → 存 refresh token。
#[tauri::command]
pub async fn spotify_connect(app: AppHandle) -> Result<String, String> {
    let cid = client_id(&app);
    if cid.is_empty() {
        return Err("尚未填入 Spotify Client ID".into());
    }

    // 產生 PKCE verifier / challenge
    let mut buf = [0u8; 48];
    getrandom::getrandom(&mut buf).map_err(|e| e.to_string())?;
    let verifier = b64url(&buf);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));

    // 組授權網址(reqwest::Url 幫忙做百分比編碼)
    let auth_url = reqwest::Url::parse_with_params(
        "https://accounts.spotify.com/authorize",
        &[
            ("client_id", cid.as_str()),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT),
            ("scope", SCOPES),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
        ],
    )
    .map_err(|e| e.to_string())?;

    app.opener()
        .open_url(auth_url.to_string(), None::<&str>)
        .map_err(|e| format!("開啟瀏覽器失敗:{e}"))?;

    // 起暫時本機伺服器接 redirect(最多等 120 秒)
    let code = tauri::async_runtime::spawn_blocking(wait_for_code)
        .await
        .map_err(|e| e.to_string())??;

    // 用 code 換 token
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT),
            ("client_id", cid.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("連線失敗:{e}"))?;
    if !resp.status().is_success() {
        let b = resp.text().await.unwrap_or_default();
        return Err(format!("授權交換失敗:{}", truncate(&b, 200)));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let access = v["access_token"].as_str().unwrap_or("").to_string();
    let refresh = v["refresh_token"].as_str().unwrap_or("").to_string();
    let expires = v["expires_in"].as_u64().unwrap_or(3600);
    if access.is_empty() || refresh.is_empty() {
        return Err("Spotify 沒有回傳完整 token".into());
    }
    keys::set_key("spotify", &refresh)?;
    cache_token(&app, &access, expires);
    Ok("已連結 Spotify ✅".into())
}

/// 暫時 HTTP 伺服器,接 OAuth 轉址、取出 ?code=。
fn wait_for_code() -> Result<String, String> {
    let server = tiny_http::Server::http("127.0.0.1:8888")
        .map_err(|e| format!("無法啟動本機授權伺服器(8888 可能被占用):{e}"))?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let html = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .map_err(|_| "header 建立失敗".to_string())?;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("授權逾時(120 秒沒完成)")?;
        match server.recv_timeout(remaining) {
            Ok(Some(req)) => {
                let url = req.url().to_string();
                if let Some(code) = url.split('?').nth(1).and_then(|q| {
                    q.split('&')
                        .find_map(|kv| kv.strip_prefix("code=").map(|c| c.to_string()))
                }) {
                    let body = "<html><body style='font-family:sans-serif;text-align:center;margin-top:60px'>授權完成 🎵 可以關掉這個分頁、回去找她了。</body></html>";
                    let _ = req.respond(
                        tiny_http::Response::from_string(body).with_header(html.clone()),
                    );
                    return Ok(code);
                } else if url.contains("error=") {
                    let _ = req.respond(tiny_http::Response::from_string("授權被取消。"));
                    return Err("使用者取消了 Spotify 授權".into());
                } else {
                    let _ = req.respond(tiny_http::Response::from_string("ok"));
                }
            }
            Ok(None) => return Err("授權逾時(120 秒沒完成)".into()),
            Err(e) => return Err(format!("授權伺服器錯誤:{e}")),
        }
    }
}

/* ---------------- 權杖管理 ---------------- */

fn cache_token(app: &AppHandle, access: &str, expires_in: u64) {
    let until = Instant::now() + Duration::from_secs(expires_in.saturating_sub(60).max(30));
    *app.state::<SpotifyState>().0.lock().unwrap() = Some((access.to_string(), until));
}

/// 取得可用的 access token:快取有效就用,過期則以 refresh token 刷新。
async fn get_token(app: &AppHandle) -> Result<String, String> {
    {
        let s = app.state::<SpotifyState>();
        let g = s.0.lock().unwrap();
        if let Some((tok, exp)) = g.as_ref() {
            if Instant::now() < *exp {
                return Ok(tok.clone());
            }
        }
    }
    let cid = client_id(app);
    let refresh = match keys::get_key("spotify") {
        Some(r) if !r.trim().is_empty() => r,
        _ => {
            eprintln!("[spotify] get_token:認證管理員裡沒有 refresh token → 尚未連結");
            return Err("尚未連結 Spotify(請到設定按「連結 Spotify」)".into());
        }
    };
    eprintln!("[spotify] get_token:刷新中(client_id={}…)", &cid.chars().take(6).collect::<String>());
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", cid.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("連線失敗:{e}"))?;
    if !resp.status().is_success() {
        let b = resp.text().await.unwrap_or_default();
        return Err(format!("Spotify 授權失效,請重新連結:{}", truncate(&b, 150)));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let access = v["access_token"].as_str().unwrap_or("").to_string();
    let expires = v["expires_in"].as_u64().unwrap_or(3600);
    if let Some(new_refresh) = v["refresh_token"].as_str() {
        if !new_refresh.is_empty() {
            let _ = keys::set_key("spotify", new_refresh);
        }
    }
    if access.is_empty() {
        return Err("刷新沒拿到 token".into());
    }
    cache_token(app, &access, expires);
    Ok(access)
}

fn map_status(status: reqwest::StatusCode, body: &str) -> String {
    match status.as_u16() {
        404 => "找不到正在運作的 Spotify——請先在手機或電腦打開 Spotify、隨便播一下,再叫我控制。".into(),
        403 => "這個操作被 Spotify 擋下了(通常是因為帳號不是 Premium,或當下狀態不允許)。".into(),
        401 => "Spotify 授權過期了,請到設定重新連結一次。".into(),
        _ => format!("Spotify 回應 {}:{}", status, truncate(body, 120)),
    }
}

async fn send_map(rb: reqwest::RequestBuilder, ok_msg: String) -> String {
    match rb.send().await {
        Ok(r) if r.status().is_success() => ok_msg,
        Ok(r) => {
            let s = r.status();
            map_status(s, &r.text().await.unwrap_or_default())
        }
        Err(e) => format!("連線失敗:{e}"),
    }
}

const NO_DEVICE: &str =
    "找不到任何 Spotify 裝置——請先在手機或電腦把 Spotify App 打開(打開就好,不必先播),再叫我一次。";

/// 取得可用裝置 ID(優先 active,否則清單第一個);完全沒裝置回 None。
async fn pick_device(client: &reqwest::Client, token: &str) -> Result<Option<String>, String> {
    let resp = client
        .get(format!("{API}/me/player/devices"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("連線失敗:{e}"))?;
    if !resp.status().is_success() {
        let s = resp.status();
        return Err(map_status(s, &resp.text().await.unwrap_or_default()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let devices = v["devices"].as_array().cloned().unwrap_or_default();
    // 診斷:把 API 實際看到的裝置印到後端 log
    let summary: Vec<String> = devices
        .iter()
        .map(|d| {
            format!(
                "{}[{},active={}]",
                d["name"].as_str().unwrap_or("?"),
                d["type"].as_str().unwrap_or("?"),
                d["is_active"].as_bool().unwrap_or(false)
            )
        })
        .collect();
    eprintln!("[spotify] 偵測到 {} 台裝置:{}", devices.len(), summary.join(", "));
    let chosen = devices
        .iter()
        .find(|d| d["is_active"].as_bool().unwrap_or(false))
        .or_else(|| devices.first());
    Ok(chosen.and_then(|d| d["id"].as_str().map(|s| s.to_string())))
}

/// 同時取得 token 與一個可用裝置;完全沒裝置時回友善提示(請先開 Spotify)。
async fn token_and_device(app: &AppHandle) -> Result<(String, String), String> {
    let token = get_token(app).await?;
    let client = reqwest::Client::new();
    match pick_device(&client, &token).await? {
        Some(dev) => Ok((token, dev)),
        None => Err(NO_DEVICE.into()),
    }
}

/* ---------------- 給 agent 工具呼叫的動作 ---------------- */

/// 播放:有 query 就搜尋並播放第一個結果;沒 query 就「繼續播放」。
pub async fn play(app: &AppHandle, query: &str) -> String {
    let (token, dev) = match token_and_device(app).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let client = reqwest::Client::new();
    let q = query.trim();
    if q.is_empty() {
        return send_map(
            client
                .put(format!("{API}/me/player/play"))
                .query(&[("device_id", dev.as_str())])
                .bearer_auth(token)
                .header("Content-Length", "0"),
            "繼續播放了~".into(),
        )
        .await;
    }
    // 先搜尋曲目
    let resp = client
        .get(format!("{API}/search"))
        .query(&[("q", q), ("type", "track"), ("limit", "1")])
        .bearer_auth(&token)
        .send()
        .await;
    let v: Value = match resp {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
        Ok(r) => return map_status(r.status(), &r.text().await.unwrap_or_default()),
        Err(e) => return format!("連線失敗:{e}"),
    };
    let item = &v["tracks"]["items"][0];
    let uri = item["uri"].as_str().unwrap_or("");
    if uri.is_empty() {
        return format!("Spotify 上找不到「{q}」這首歌。");
    }
    let name = item["name"].as_str().unwrap_or("");
    let artist = item["artists"][0]["name"].as_str().unwrap_or("");
    send_map(
        client
            .put(format!("{API}/me/player/play"))
            .query(&[("device_id", dev.as_str())])
            .bearer_auth(token)
            .json(&json!({ "uris": [uri] })),
        format!("開始播放:{artist} - {name} 🎵"),
    )
    .await
}

pub async fn pause(app: &AppHandle) -> String {
    let (token, dev) = match token_and_device(app).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    send_map(
        reqwest::Client::new()
            .put(format!("{API}/me/player/pause"))
            .query(&[("device_id", dev.as_str())])
            .bearer_auth(token)
            .header("Content-Length", "0"),
        "暫停了~".into(),
    )
    .await
}

pub async fn next(app: &AppHandle) -> String {
    let (token, dev) = match token_and_device(app).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    send_map(
        reqwest::Client::new()
            .post(format!("{API}/me/player/next"))
            .query(&[("device_id", dev.as_str())])
            .bearer_auth(token)
            .header("Content-Length", "0"),
        "幫你跳下一首了~".into(),
    )
    .await
}

pub async fn previous(app: &AppHandle) -> String {
    let (token, dev) = match token_and_device(app).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    send_map(
        reqwest::Client::new()
            .post(format!("{API}/me/player/previous"))
            .query(&[("device_id", dev.as_str())])
            .bearer_auth(token)
            .header("Content-Length", "0"),
        "回到上一首了~".into(),
    )
    .await
}

pub async fn set_volume(app: &AppHandle, percent: i64) -> String {
    let p = percent.clamp(0, 100);
    let (token, dev) = match token_and_device(app).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    send_map(
        reqwest::Client::new()
            .put(format!("{API}/me/player/volume"))
            .query(&[("volume_percent", p.to_string()), ("device_id", dev)])
            .bearer_auth(token)
            .header("Content-Length", "0"),
        format!("音量調到 {p}% 了~"),
    )
    .await
}

/// 給前端輪詢:目前是否在播放 + 試著取得這首歌的 BPM(拿不到回 null)。
/// 回傳 { "playing": bool, "bpm": number|null }。任何錯誤都安靜地回不在播放。
#[tauri::command]
pub async fn spotify_playback_state(app: AppHandle) -> Value {
    let token = match get_token(&app).await {
        Ok(t) => t,
        Err(_) => return json!({ "playing": false, "bpm": null }),
    };
    let client = reqwest::Client::new();
    let v: Value = match client
        .get(format!("{API}/me/player"))
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
        _ => return json!({ "playing": false, "bpm": null }), // 204=沒裝置/沒在播
    };
    let playing = v["is_playing"].as_bool().unwrap_or(false);
    let track_id = v["item"]["id"].as_str().unwrap_or("");
    let mut bpm = json!(null);
    if playing && !track_id.is_empty() {
        // audio-features 可能被 Spotify 新政策擋(403)→ 靜默忽略,用預設節奏
        if let Ok(r) = client
            .get(format!("{API}/audio-features/{track_id}"))
            .bearer_auth(&token)
            .send()
            .await
        {
            if r.status().is_success() {
                if let Ok(af) = r.json::<Value>().await {
                    if let Some(t) = af["tempo"].as_f64() {
                        bpm = json!(t);
                    }
                }
            }
        }
    }
    json!({ "playing": playing, "bpm": bpm })
}

/// 相對調整音量(delta 可正可負),會先讀目前音量。
pub async fn adjust_volume(app: &AppHandle, delta: i64) -> String {
    let token = match get_token(app).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let client = reqwest::Client::new();
    let cur = match client
        .get(format!("{API}/me/player"))
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            let v: Value = r.json().await.unwrap_or_default();
            v["device"]["volume_percent"].as_i64().unwrap_or(50)
        }
        Ok(r) if r.status().as_u16() == 204 => 50,
        Ok(r) => return map_status(r.status(), &r.text().await.unwrap_or_default()),
        Err(e) => return format!("連線失敗:{e}"),
    };
    set_volume(app, cur + delta).await
}
