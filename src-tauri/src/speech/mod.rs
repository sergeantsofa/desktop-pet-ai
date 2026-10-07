//! M2 後半:語音 sidecar 管理(Piper TTS / Whisper.cpp STT)。
//!
//! 二進位與模型放在 app_data_dir/speech/:
//!   speech/piper/piper.exe + <語音>.onnx(+ 同名 .onnx.json)
//!   speech/whisper/whisper-cli.exe(或舊版 main.exe)+ ggml-*.bin
//!
//! 取得方式(任一):
//!   1. 設定面板的「一鍵安裝語音元件」(走 [`setup`] 模組,安裝版使用者用這個)
//!   2. `scripts\setup-speech.ps1`(從原始碼開發時方便)
//! 找不到時前端自動退回 Web Speech API(speechSynthesis)。

pub mod setup;

use base64::Engine as _;
use serde::Serialize;
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

/// 隱藏子行程的主控台視窗(Windows CREATE_NO_WINDOW)
pub(crate) fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    #[cfg(not(windows))]
    let _ = cmd;
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechStatus {
    /// sidecar 安裝目錄(顯示給使用者)
    pub dir: String,
    pub piper: bool,
    pub piper_voice: Option<String>,
    pub whisper: bool,
    pub whisper_model: Option<String>,
}

fn speech_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|d| d.join("speech"))
        .map_err(|e| e.to_string())
}

fn find_file(dir: &Path, pred: impl Fn(&str) -> bool) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if pred(&name) && entry.path().is_file() {
            return Some(entry.path());
        }
    }
    None
}

fn piper_exe(dir: &Path) -> Option<PathBuf> {
    let p = dir.join("piper").join("piper.exe");
    p.is_file().then_some(p)
}

fn piper_voice(dir: &Path) -> Option<PathBuf> {
    find_file(&dir.join("piper"), |n| n.ends_with(".onnx"))
}

fn whisper_exe(dir: &Path) -> Option<PathBuf> {
    ["whisper-cli.exe", "main.exe"]
        .iter()
        .map(|n| dir.join("whisper").join(n))
        .find(|p| p.is_file())
}

fn whisper_model(dir: &Path) -> Option<PathBuf> {
    find_file(&dir.join("whisper"), |n| n.starts_with("ggml-") && n.ends_with(".bin"))
}

fn file_name(p: &Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().into_owned())
}

fn temp_file(prefix: &str, ext: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.{ext}", std::process::id()))
}

/* ---------------- Tauri 命令 ---------------- */

#[tauri::command]
pub fn speech_status(app: AppHandle) -> Result<SpeechStatus, String> {
    let dir = speech_dir(&app)?;
    let voice = piper_voice(&dir);
    let model = whisper_model(&dir);
    Ok(SpeechStatus {
        dir: dir.display().to_string(),
        piper: piper_exe(&dir).is_some() && voice.is_some(),
        piper_voice: voice.as_deref().and_then(file_name),
        whisper: whisper_exe(&dir).is_some() && model.is_some(),
        whisper_model: model.as_deref().and_then(file_name),
    })
}

/// Piper 合成:回傳 WAV 原始位元組(前端做 Blob URL 餵 model.speak 對嘴)。
/// length_scale 控制語速(值越大越慢;前端以 1/rate 換算)。
#[tauri::command]
pub async fn tts_synthesize(
    app: AppHandle,
    text: String,
    length_scale: Option<f32>,
) -> Result<tauri::ipc::Response, String> {
    let dir = speech_dir(&app)?;
    let exe = piper_exe(&dir)
        .ok_or("找不到 piper.exe。請到「設定 → 語音」按「一鍵安裝語音元件」,或把 piper 放進 speech\\piper\\")?;
    let voice = piper_voice(&dir).ok_or("找不到 Piper 語音模型(.onnx)")?;
    let ls = length_scale.unwrap_or(1.0).clamp(0.5, 2.0);

    let wav = tauri::async_runtime::spawn_blocking(move || run_piper(&exe, &voice, &text, ls))
        .await
        .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(wav))
}

fn run_piper(exe: &Path, voice: &Path, text: &str, length_scale: f32) -> Result<Vec<u8>, String> {
    let out = temp_file("pet-tts", "wav");
    let mut cmd = Command::new(exe);
    cmd.arg("-m")
        .arg(voice)
        .arg("-f")
        .arg(&out)
        .arg("--length_scale")
        .arg(length_scale.to_string())
        // espeak-ng-data 等資源以 exe 所在目錄為基準
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| format!("piper 啟動失敗:{e}"))?;
    // 一行一句;換行會被 piper 當多段,壓成單行
    let line = text.replace(['\r', '\n'], " ");
    child
        .stdin
        .take()
        .ok_or("無法取得 piper stdin")?
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        let _ = fs::remove_file(&out);
        return Err(format!("piper 結束碼異常:{status}"));
    }
    let bytes = fs::read(&out).map_err(|e| format!("讀取合成結果失敗:{e}"))?;
    let _ = fs::remove_file(&out);
    Ok(bytes)
}

/// Edge 神經網路語音合成:回傳 MP3 bytes(免金鑰、需網路;聲線自然甜美)。
/// voice 用短名如 "zh-CN-XiaoyiNeural";rate 1.0 = 原速。
#[tauri::command]
pub async fn tts_edge(
    text: String,
    voice: String,
    rate: Option<f32>,
) -> Result<tauri::ipc::Response, String> {
    // Edge 的 rate 參數是 -100..100 的百分比
    let rate_pct = ((rate.unwrap_or(1.0).clamp(0.5, 2.0) - 1.0) * 100.0) as i32;
    let bytes = tauri::async_runtime::spawn_blocking(move || run_edge(&text, &voice, rate_pct))
        .await
        .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Edge TTS 合成的底層實作。`pub(crate)` 讓區網遠端聊天(remote.rs 的 /api/tts)也能用同款甜美語音。
pub(crate) fn run_edge(text: &str, voice: &str, rate_pct: i32) -> Result<Vec<u8>, String> {
    use msedge_tts::tts::{client::connect, SpeechConfig};

    // 短名 "zh-CN-XiaoyiNeural" → 端點要的完整名稱
    let (locale, name) = voice
        .rsplit_once('-')
        .ok_or_else(|| format!("語音名稱格式錯誤:{voice}"))?;
    let voice_name =
        format!("Microsoft Server Speech Text to Speech Voice ({locale}, {name})");

    let config = SpeechConfig {
        voice_name,
        audio_format: "audio-24khz-48kbitrate-mono-mp3".into(),
        pitch: 0,
        rate: rate_pct,
        volume: 0,
    };
    let mut tts = connect().map_err(|e| format!("Edge TTS 連線失敗:{e}"))?;
    let audio = tts
        .synthesize(text, &config)
        .map_err(|e| format!("Edge TTS 合成失敗:{e}"))?;
    if audio.audio_bytes.is_empty() {
        return Err("Edge TTS 回傳空音訊".into());
    }
    Ok(audio.audio_bytes)
}

/// Fish Speech 合成:HTTP API 模式(本地 Fish Speech FastAPI 服務)。
/// POST /v1/tts → 回傳 WAV bytes → 前端 model.speak 對嘴。
/// `ref_audio` 與 `ref_text` 為選擇性的參考音訊(台灣腔範本)。
#[tauri::command]
pub async fn tts_fish(
    text: String,
    api_url: Option<String>,
    ref_audio: Option<String>,
    ref_text: Option<String>,
) -> Result<tauri::ipc::Response, String> {
    let url = api_url
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8080/v1/tts".into());

    let mut body = serde_json::json!({
        "text": text,
        "format": "wav",
    });
    // 參考音色(台灣腔範本):Fish /v1/tts 的 ServeTTSRequest 要 references=[{audio, text}],
    // JSON 模式下 audio 是「WAV 的 base64」(伺服器會自動 b64decode),不是檔案路徑。
    if let Some(ra) = ref_audio.filter(|s| !s.is_empty()) {
        match std::fs::read(&ra) {
            Ok(bytes) => {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                body["references"] = serde_json::json!([{
                    "audio": b64,
                    "text": ref_text.unwrap_or_default(),
                }]);
            }
            Err(e) => return Err(format!("讀取參考音訊失敗({ra}):{e}")),
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("建立 HTTP client 失敗:{e}"))?;

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Fish Speech API 連線失敗(服務有開嗎?):{e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!("Fish Speech API 回傳 {status}: {body_text}"));
    }

    let wav = resp
        .bytes()
        .await
        .map_err(|e| format!("讀取 Fish Speech 音訊失敗:{e}"))?;

    if wav.is_empty() {
        return Err("Fish Speech 回傳空音訊".into());
    }

    Ok(tauri::ipc::Response::new(wav.to_vec()))
}

#[cfg(test)]
mod tests {
    /// 需要網路;手動執行:cargo test edge -- --ignored --nocapture
    #[test]
    #[ignore]
    fn edge_synthesize() {
        let bytes = super::run_edge("嗨!我是桌面上的小夥伴,今天也要加油喔!", "zh-CN-XiaoyiNeural", 0)
            .expect("edge tts failed");
        println!("mp3 bytes: {}", bytes.len());
        assert!(bytes.len() > 1000);
        let out = std::env::temp_dir().join("edge-test.mp3");
        std::fs::write(&out, &bytes).unwrap();
        println!("written: {}", out.display());
    }
}

/// Whisper 辨識:輸入 16kHz 單聲道 PCM16 WAV(base64),回傳文字。
#[tauri::command]
pub async fn stt_transcribe(app: AppHandle, wav_b64: String) -> Result<String, String> {
    let dir = speech_dir(&app)?;
    let exe = whisper_exe(&dir).ok_or(
        "找不到 whisper-cli.exe。請到「設定 → 語音」按「一鍵安裝語音元件」,或把 whisper.cpp 放進 speech\\whisper\\",
    )?;
    let model = whisper_model(&dir).ok_or(
        "找不到 Whisper 模型(ggml-*.bin)。請到「設定 → 語音」按「一鍵安裝語音元件」",
    )?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(wav_b64)
        .map_err(|e| format!("音訊解碼失敗:{e}"))?;

    tauri::async_runtime::spawn_blocking(move || run_whisper(&exe, &model, &bytes))
        .await
        .map_err(|e| e.to_string())?
}

fn run_whisper(exe: &Path, model: &Path, wav: &[u8]) -> Result<String, String> {
    let input = temp_file("pet-stt", "wav");
    fs::write(&input, wav).map_err(|e| e.to_string())?;

    let mut cmd = Command::new(exe);
    cmd.arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&input)
        // -nt 不輸出時間戳、-np 不輸出進度、-sns 抑制非語音 token(減少靜音時的幻覺)。
        // 不給 --prompt:prompt 反而容易讓模型在靜音時「接龍」出字幕幻覺。
        .args(["-nt", "-np", "-sns", "-l", "zh"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut cmd);

    let output = cmd.output();
    let _ = fs::remove_file(&input);
    let output = output.map_err(|e| format!("whisper 啟動失敗:{e}"))?;
    if !output.status.success() {
        return Err(format!("whisper 結束碼異常:{}", output.status));
    }
    let text = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(if is_hallucination(&text) { String::new() } else { text })
}

/// Whisper 在靜音/雜訊時常吐出訓練資料(YouTube 字幕)殘留的幻覺片語。
/// 命中就視為「沒聽到」,回空字串讓前端提示再說一次。
fn is_hallucination(text: &str) -> bool {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return true;
    }
    const PHRASES: &[&str] = &[
        "字幕", "訂閱", "订阅", "點贊", "点赞", "按讚", "點讚", "謝謝觀看", "谢谢观看",
        "謝謝大家", "谢谢大家", "請不吝", "请不吝", "明鏡", "明镜", "點點欄目", "点点栏目",
        "獨播劇場", "独播剧场", "轉發", "转发", "打賞", "打赏", "下次再見", "下次再见",
        "關注", "关注", "Amara", "字幕志願", "字幕志愿", "MING PAO",
    ];
    // 短句(<= 12 字)且含任一幻覺片語 → 視為幻覺;這類片語幾乎不會出現在對桌寵的日常口語
    let hit = PHRASES.iter().any(|p| t.contains(p));
    hit && t.chars().count() <= 12
}
