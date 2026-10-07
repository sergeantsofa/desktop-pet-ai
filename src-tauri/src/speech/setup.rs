//! 離線語音元件一鍵安裝(Whisper 語音輸入 + Piper 朗讀)。
//!
//! 為什麼放在 App 內而不是只給 PowerShell 腳本:
//! 安裝版使用者根本拿不到 `scripts\setup-speech.ps1`,遇到「找不到 whisper-cli.exe」
//! 只能看著一句「請執行 setup-speech.ps1」發呆。這裡做成跟「一鍵下載範例角色」一樣的體驗。
//!
//! 安裝位置:`%APPDATA%\com.desktoppet.ai\speech\`
//!   whisper\whisper-cli.exe + ggml-<model>.bin
//!   piper\piper.exe + *.onnx(+ .onnx.json)
//!
//! 實作刻意**不新增任何 crate 相依**:
//!   - 下載用 Windows 內建的 `curl.exe`(`--progress-bar` 的 % 直接轉成進度事件)
//!   - 解壓用 Windows 內建的 `tar.exe`(實測可解 zip)

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tauri::{AppHandle, Emitter, Manager};

use super::hide_console;

/// Whisper 模型可選大小(tiny 最快、medium 最準)
const WHISPER_MODELS: &[&str] = &["tiny", "base", "small", "medium"];

/// GitHub API 需要 User-Agent,不然會被擋
const UA: &str = "desktop-pet-ai";

fn speech_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("speech");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建立 speech 目錄失敗:{e}"))?;
    Ok(dir)
}

fn emit(app: &AppHandle, done: u32, total: u32, label: &str) {
    let _ = app.emit(
        "speech-setup-progress",
        serde_json::json!({ "done": done, "total": total, "label": label }),
    );
}

/// 一鍵安裝離線語音元件。
///
/// - `whisper` / `piper` 可分別指定要不要裝(預設都裝)。
/// - `whisper_model`:`tiny` / `base`(預設)/ `small` / `medium`。
/// - 進度走 `speech-setup-progress` 事件(`done`/`total`/`label`)。
#[tauri::command]
pub async fn setup_speech(
    app: AppHandle,
    whisper: Option<bool>,
    piper: Option<bool>,
    whisper_model: Option<String>,
) -> Result<String, String> {
    let want_whisper = whisper.unwrap_or(true);
    let want_piper = piper.unwrap_or(true);
    let model = whisper_model.unwrap_or_else(|| "base".to_string());
    if !WHISPER_MODELS.contains(&model.as_str()) {
        return Err(format!(
            "不支援的 Whisper 模型「{model}」,可用:{:?}",
            WHISPER_MODELS
        ));
    }

    let dir = speech_dir(&app)?;
    let whisper_dir = dir.join("whisper");
    let piper_dir = dir.join("piper");

    // 4 個階段:whisper 執行檔、whisper 模型、piper 執行檔、piper 語音
    let mut steps: Vec<&str> = Vec::new();
    if want_whisper {
        steps.push("whisper-exe");
        steps.push("whisper-model");
    }
    if want_piper {
        steps.push("piper-exe");
        steps.push("piper-voice");
    }
    let total = steps.len() as u32;
    let mut done: u32 = 0;
    let mut notes: Vec<String> = Vec::new();

    // ---- Whisper 執行檔 ----
    if want_whisper {
        std::fs::create_dir_all(&whisper_dir).map_err(|e| e.to_string())?;
        if has_whisper_exe(&whisper_dir) {
            notes.push("Whisper 執行檔:已存在,略過".into());
        } else {
            emit(&app, done, total, "查詢 Whisper 版本…");
            let url = latest_whisper_zip().await?;
            emit(&app, done, total, "下載 Whisper 執行檔…");
            let zip = dir.join("_whisper.zip");
            download(&app, &url, &zip, done, total)?;
            emit(&app, done, total, "解壓 Whisper…");
            extract_zip(&zip, &whisper_dir)?;
            let _ = std::fs::remove_file(&zip);
            if !has_whisper_exe(&whisper_dir) {
                return Err("解壓後仍找不到 whisper-cli.exe,請回報此問題".into());
            }
            notes.push("Whisper 執行檔:安裝完成".into());
        }
        done += 1;
    }

    // ---- Whisper 模型 ----
    if want_whisper {
        let bin = whisper_dir.join(format!("ggml-{model}.bin"));
        if has_whisper_model(&whisper_dir) {
            notes.push("Whisper 模型:已存在,略過".into());
        } else {
            let url = format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin"
            );
            emit(&app, done, total, &format!("下載 Whisper 模型({model},約 150MB)…"));
            download(&app, &url, &bin, done, total)?;
            notes.push(format!("Whisper 模型 ggml-{model}.bin:安裝完成"));
        }
        done += 1;
    }

    // ---- Piper 執行檔 ----
    if want_piper {
        std::fs::create_dir_all(&piper_dir).map_err(|e| e.to_string())?;
        if piper_dir.join("piper.exe").is_file() {
            notes.push("Piper 執行檔:已存在,略過".into());
        } else {
            emit(&app, done, total, "查詢 Piper 版本…");
            let url = latest_piper_zip().await?;
            emit(&app, done, total, "下載 Piper…");
            let zip = dir.join("_piper.zip");
            download(&app, &url, &zip, done, total)?;
            emit(&app, done, total, "解壓 Piper…");
            extract_zip(&zip, &piper_dir)?;
            let _ = std::fs::remove_file(&zip);
            if !piper_dir.join("piper.exe").is_file() {
                return Err("解壓後仍找不到 piper.exe,請回報此問題".into());
            }
            notes.push("Piper 執行檔:安裝完成".into());
        }
        done += 1;
    }

    // ---- Piper 中文語音 ----
    if want_piper {
        if has_piper_voice(&piper_dir) {
            notes.push("Piper 中文語音:已存在,略過".into());
        } else {
            let base =
                "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/zh/zh_CN/huayan/medium";
            emit(&app, done, total, "下載 Piper 中文語音(約 60MB)…");
            download(
                &app,
                &format!("{base}/zh_CN-huayan-medium.onnx"),
                &piper_dir.join("zh_CN-huayan-medium.onnx"),
                done,
                total,
            )?;
            download(
                &app,
                &format!("{base}/zh_CN-huayan-medium.onnx.json"),
                &piper_dir.join("zh_CN-huayan-medium.onnx.json"),
                done,
                total,
            )?;
            notes.push("Piper 中文語音 zh_CN-huayan-medium:安裝完成".into());
        }
    }

    emit(&app, done, total, "完成!");
    Ok(notes.join("\n"))
}

fn has_whisper_exe(dir: &Path) -> bool {
    ["whisper-cli.exe", "main.exe"]
        .iter()
        .any(|n| dir.join(n).is_file())
}

fn has_whisper_model(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.starts_with("ggml-") && n.ends_with(".bin")
            })
        })
        .unwrap_or(false)
}

fn has_piper_voice(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".onnx"))
        })
        .unwrap_or(false)
}

/// 找「真的有 Windows 執行檔」的最新 whisper.cpp release。
///
/// ⚠️ 不能只看 `releases/latest`:那裡常常只有版號、沒有任何 asset
/// (例如 v1.9.5),舊的 setup-speech.ps1 就是這樣壞掉的。
/// 所以逐一往回找,直到找到帶 `whisper-bin-x64.zip` 的那一版。
async fn latest_whisper_zip() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| e.to_string())?;
    let rels: serde_json::Value = client
        .get("https://api.github.com/repos/ggml-org/whisper.cpp/releases?per_page=20")
        .send()
        .await
        .map_err(|e| format!("查詢 whisper.cpp 版本失敗:{e}"))?
        .json()
        .await
        .map_err(|e| format!("解析 whisper.cpp 版本失敗:{e}"))?;

    let arr = rels.as_array().ok_or("whisper.cpp releases 格式異常")?;
    for rel in arr {
        if let Some(assets) = rel["assets"].as_array() {
            for a in assets {
                if a["name"].as_str() == Some("whisper-bin-x64.zip") {
                    if let Some(url) = a["browser_download_url"].as_str() {
                        return Ok(url.to_string());
                    }
                }
            }
        }
    }
    Err("在 whisper.cpp 找不到含 Windows 執行檔的版本,請稍後再試或手動放入 speech\\whisper\\".into())
}

/// 找 Piper 的 Windows 套件(同樣逐一往回找,避免 latest 沒有 asset)
async fn latest_piper_zip() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| e.to_string())?;
    let rels: serde_json::Value = client
        .get("https://api.github.com/repos/rhasspy/piper/releases?per_page=20")
        .send()
        .await
        .map_err(|e| format!("查詢 Piper 版本失敗:{e}"))?
        .json()
        .await
        .map_err(|e| format!("解析 Piper 版本失敗:{e}"))?;

    let arr = rels.as_array().ok_or("Piper releases 格式異常")?;
    for rel in arr {
        if let Some(assets) = rel["assets"].as_array() {
            for a in assets {
                let name = a["name"].as_str().unwrap_or("");
                if name.contains("windows_amd64") && name.ends_with(".zip") {
                    if let Some(url) = a["browser_download_url"].as_str() {
                        return Ok(url.to_string());
                    }
                }
            }
        }
    }
    Err("在 Piper 找不到 Windows 套件,請稍後再試或手動放入 speech\\piper\\".into())
}

/// 用 curl.exe 下載;`--progress-bar` 的百分比轉成進度事件。
///
/// 不新增相依的理由:curl 是 Windows 內建;而且它對大檔 / 轉址 / 重試的處理
/// 比手寫 reqwest 串流可靠(參考 setup-speech.ps1 的註解)。
fn download(app: &AppHandle, url: &str, dest: &Path, done: u32, total: u32) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let label = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "下載中".into());

    let mut cmd = Command::new("curl.exe");
    cmd.arg("-L") // 跟著轉址(HuggingFace / GitHub 都會轉)
        .arg("--fail")
        .arg("--retry")
        .arg("3")
        .arg("--progress-bar")
        .arg("-o")
        .arg(dest)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| format!("無法啟動 curl.exe:{e}"))?;

    if let Some(err) = child.stderr.take() {
        let mut reader = BufReader::new(err);
        let mut line = Vec::new();
        loop {
            line.clear();
            // curl 的進度是用 \r 更新的,所以不能只切 \n
            match reader.read_until(b'\r', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let s = String::from_utf8_lossy(&line);
                    if let Some(pct) = parse_percent(&s) {
                        emit(app, done, total, &format!("{label} {pct}%"));
                    }
                }
                Err(_) => break,
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("下載失敗({status}):{url}"));
    }
    Ok(())
}

/// 從 curl 進度列抓百分比(例如 `####  45.2%`)
fn parse_percent(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'%' {
            // 往回找數字
            let mut j = i;
            while j > 0 && (bytes[j - 1].is_ascii_digit() || bytes[j - 1] == b'.') {
                j -= 1;
            }
            let num = s[j..i].trim_end_matches('.');
            if let Ok(v) = num.parse::<f64>() {
                return Some(v.round() as u32);
            }
        }
    }
    None
}

/// 用 Windows 內建的 tar.exe 解 zip/壓縮檔,再把「執行檔所在的資料夾」整包搬過去。
///
/// 為什麼要先解到暫存再搬:whisper / piper 的壓縮檔內多一層目錄,
/// 而且 piper 需要同層的 dll 與 espeak-ng-data,必須整夾搬。
fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!(
        "pet-speech-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    let mut cmd = Command::new("tar.exe");
    cmd.arg("-xf")
        .arg(zip)
        .arg("-C")
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| format!("無法啟動 tar.exe:{e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("解壓失敗(tar.exe)".into());
    }

    // 找出執行檔所在的資料夾
    let marker = ["whisper-cli.exe", "main.exe", "piper.exe"]
        .iter()
        .find_map(|m| find_file(&tmp, m));

    let src_dir = match marker {
        Some(exe) => exe.parent().map(|p| p.to_path_buf()),
        None => None,
    };
    let src_dir = src_dir.unwrap_or_else(|| tmp.clone());

    copy_dir_contents(&src_dir, dest).map_err(|e| format!("複製檔案失敗:{e}"))?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            // 別遞迴太深(壓縮檔最多兩三層)
            if let Some(found) = find_file_limited(&p, name, 4) {
                return Some(found);
            }
        } else if e.file_name().to_string_lossy() == name {
            return Some(p);
        }
    }
    None
}

fn find_file_limited(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file_limited(&p, name, depth - 1) {
                return Some(f);
            }
        } else if e.file_name().to_string_lossy() == name {
            return Some(p);
        }
    }
    None
}

fn copy_dir_contents(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let from = e.path();
        let to = dest.join(e.file_name());
        if from.is_dir() {
            copy_dir_contents(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_curl_percent() {
        assert_eq!(parse_percent("#########            45.2%"), Some(45));
        assert_eq!(parse_percent("100.0%"), Some(100));
        assert_eq!(parse_percent("  0.0%"), Some(0));
        assert_eq!(parse_percent("no percent here"), None);
    }

    #[test]
    fn finds_nested_executable() {
        let base = std::env::temp_dir().join(format!("pet-speech-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let nested = base.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("whisper-cli.exe"), b"x").unwrap();
        let found = find_file(&base, "whisper-cli.exe");
        assert!(found.is_some(), "應該要能找到巢狀的可執行檔");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn copies_directory_tree() {
        let base = std::env::temp_dir().join(format!("pet-speech-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("piper.exe"), b"exe").unwrap();
        std::fs::write(src.join("sub").join("voice.onnx"), b"onnx").unwrap();

        copy_dir_contents(&src, &dst).unwrap();
        assert!(dst.join("piper.exe").is_file());
        assert!(dst.join("sub").join("voice.onnx").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }
}
