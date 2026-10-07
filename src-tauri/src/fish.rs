//! Fish Speech 本地 API server 的生命週期管理(可選的自動啟動)。
//!
//! 桌寵本身不內建 Fish(它是吃 CUDA 的 Python 服務),這裡只是「幫你把你設定好的
//! 啟動指令跑起來、結束時關掉」。實際合成走 `speech::tts_fish`(HTTP 客戶端)。
//! 啟動前會先探測 API 位址,已在執行就不重複啟動。

use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// 由桌寵啟動的 Fish 子行程(若有)。結束時用來把它關掉。
pub struct FishState(pub Mutex<Option<Child>>);

impl Default for FishState {
    fn default() -> Self {
        FishState(Mutex::new(None))
    }
}

/// 桌寵啟動時呼叫:若有開自動啟動且設了指令就把 Fish 跑起來。回傳人類可讀狀態。
pub fn try_start(app: &AppHandle) -> String {
    let s = crate::llm::load_settings(app);
    if !s.fish_autostart {
        return "Fish 自動啟動未開啟".into();
    }
    start_with(app, &s.fish_launch_cmd, &s.fish_cwd, &s.fish_api_base)
}

/// 依指定指令/工作目錄啟動 Fish。已在執行(API 探測得到)或我們已啟動過就略過。
pub fn start_with(app: &AppHandle, cmd: &str, cwd: &str, api_base: &str) -> String {
    if cmd.trim().is_empty() {
        return "未設定 Fish 啟動指令".into();
    }
    if is_reachable(api_base) {
        return format!("Fish 已在執行({api_base}),略過啟動");
    }
    let state: tauri::State<FishState> = app.state();
    let mut guard = state.0.lock().unwrap();
    if let Some(child) = guard.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return "Fish 已由桌寵啟動中".into();
        }
    }
    // 用 cmd /C 跑整行指令(支援 conda run / python -m … 等);隱藏主控台。
    let mut c = Command::new("cmd");
    c.arg("/C").arg(cmd);
    if !cwd.trim().is_empty() {
        c.current_dir(cwd);
    }
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    crate::speech::hide_console(&mut c);
    match c.spawn() {
        Ok(child) => {
            *guard = Some(child);
            format!("已啟動 Fish(指令:{cmd})。模型載入要數十秒,稍候再試聽。")
        }
        Err(e) => format!("Fish 啟動失敗:{e}"),
    }
}

/// 關掉由桌寵啟動的 Fish(連同 cmd 底下的 python 子程序整棵樹)。
pub fn stop(app: &AppHandle) {
    let state: tauri::State<FishState> = app.state();
    let mut guard = state.0.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let pid = child.id();
        // cmd /C 下的 python 是子程序,要用 taskkill /T 砍整棵樹才不會留孤兒佔 GPU
        let mut k = Command::new("taskkill");
        k.args(["/PID", &pid.to_string(), "/T", "/F"]);
        crate::speech::hide_console(&mut k);
        let _ = k.status();
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// API 根位址能不能連上(TCP 短逾時),用來判斷 Fish 是否已在跑。
fn is_reachable(api_base: &str) -> bool {
    let Some((host, port)) = parse_host_port(api_base) else {
        return false;
    };
    if let Ok(mut addrs) = (host.as_str(), port).to_socket_addrs() {
        if let Some(addr) = addrs.next() {
            return TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok();
        }
    }
    false
}

fn parse_host_port(url: &str) -> Option<(String, u16)> {
    let s = url.trim().trim_start_matches("http://").trim_start_matches("https://");
    let s = s.split('/').next().unwrap_or(s);
    let mut it = s.split(':');
    let host = it.next()?.trim().to_string();
    if host.is_empty() {
        return None;
    }
    let port = it.next().and_then(|p| p.trim().parse().ok()).unwrap_or(80);
    Some((host, port))
}

/* ---------------- Tauri 命令 ---------------- */

/// 手動啟動 Fish(設定面板「啟動 Fish」按鈕)。
#[tauri::command]
pub fn fish_start(app: AppHandle) -> String {
    let s = crate::llm::load_settings(&app);
    start_with(&app, &s.fish_launch_cmd, &s.fish_cwd, &s.fish_api_base)
}

/// 手動停止由桌寵啟動的 Fish。
#[tauri::command]
pub fn fish_stop(app: AppHandle) -> String {
    stop(&app);
    "已要求停止 Fish".into()
}
