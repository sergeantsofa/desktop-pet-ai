//! 本機唯讀檔案伺服器:把外部資源資料夾(%APPDATA%\com.desktoppet.ai\)
//! 透過 http://127.0.0.1:<port>/ 提供給 webview。
//!
//! 為什麼不用 asset:// 協定:Live2D 載入器會依 model3.json 的相對路徑連帶
//! 抓 moc3/材質/表情,asset:// 對相對路徑 + 中文檔名相容性不佳(實測 Network error)。
//! 標準 http URL 與開發時的 vite server 行為一致,最穩。

use std::{
    fs,
    path::{Component, Path, PathBuf},
    thread,
};
use tiny_http::{Header, Response, Server};

/// 本機資源伺服器的 port(0 = 未啟動,前端會退回打包資源)
pub struct ResourcePort(pub u16);

#[tauri::command]
pub fn resource_port(state: tauri::State<ResourcePort>) -> u16 {
    state.0
}

/// 啟動伺服器(背景執行緒),回傳實際綁定的 port。
pub fn start(root: PathBuf) -> std::io::Result<u16> {
    let server = Server::http("127.0.0.1:0").map_err(|e| std::io::Error::other(e.to_string()))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| std::io::Error::other("無法取得 port"))?;

    thread::spawn(move || {
        for req in server.incoming_requests() {
            handle(req, &root);
        }
    });
    Ok(port)
}

fn handle(req: tiny_http::Request, root: &Path) {
    let cors = Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();
    match resolve(root, req.url()) {
        Some(path) => match fs::read(&path) {
            Ok(bytes) => {
                let ct = content_type(&path);
                let ctype = Header::from_bytes(&b"Content-Type"[..], ct.as_bytes()).unwrap();
                let resp = Response::from_data(bytes).with_header(ctype).with_header(cors);
                let _ = req.respond(resp);
            }
            Err(_) => {
                let _ = req.respond(Response::from_string("not found").with_status_code(404).with_header(cors));
            }
        },
        None => {
            let _ = req.respond(Response::from_string("bad path").with_status_code(403).with_header(cors));
        }
    }
}

/// 解析 URL → root 內的真實檔案路徑;防目錄穿越,且必須落在 root 內。
fn resolve(root: &Path, url: &str) -> Option<PathBuf> {
    let path_part = url.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode(path_part);
    let rel = decoded.trim_start_matches('/');

    let mut out = root.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(seg) => out.push(seg),
            // 拒絕 .. / 絕對路徑等
            _ => return None,
        }
    }
    let canon_root = fs::canonicalize(root).ok()?;
    let canon = fs::canonicalize(&out).ok()?;
    canon.starts_with(&canon_root).then_some(canon)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()).as_deref() {
        Some("json") => "application/json; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("moc3") | Some("bin") => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

/// 最小 percent-decode(處理中文檔名等 %XX)
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
