//! OpenAI Chat Completions 相容串流客戶端 + 自動降級 + Agent 工具迴圈(M3)。

use crate::llm::{keys, CancelState, ChatMessage, Persona, ProviderCfg, Settings, TaskRoute};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 情緒標籤集合(規格 §4.2;前端據此切換 Live2D 表情)
const EMOTION_TAGS: &str = "[happy] [sad] [angry] [surprised] [shy] [sleepy] [neutral]";

/// Agent 工具迴圈上限(避免模型無限呼叫工具)
const MAX_TOOL_ROUNDS: usize = 4;

fn system_prompt(persona: &Persona, tools_enabled: bool, memories: &str) -> String {
    let tools_hint = if tools_enabled {
        "\n你可以視需要使用提供的工具(查時間、讀剪貼簿、開網頁、看系統狀態);\
         用完工具要把結果口語化地轉述,不要原樣照唸。\
         聽到值得長期記住的事(使用者的喜好、身分、約定)就用 save_memory 記下來;\
         想不起來的舊事用 search_memory 找;使用者要你忘記就用 forget_memory。"
    } else {
        ""
    };
    format!(
        "你是「{name}」,一個桌面寵物助理。{prompt}\n\n\
         回應規則:每次回應的最開頭,先輸出一個最符合你當下情緒的標籤,\
         從 {tags} 中擇一,然後直接接回應內容,例如「[happy]今天天氣超好的!」。\
         整段回應只能有這一個標籤,不要用 markdown,保持口語、簡短。{tools_hint}{memories}",
        name = persona.name,
        prompt = persona.system_prompt,
        tags = EMOTION_TAGS,
    )
}

pub async fn run_chat(
    app: AppHandle,
    settings: Settings,
    request_id: String,
    task: String,
    messages: Vec<ChatMessage>,
    persist: bool,
) -> Result<(), String> {
    let route = settings
        .routing
        .get(&task)
        .or_else(|| settings.routing.get("chat"))
        .cloned()
        .ok_or("找不到任務路由設定")?;
    let provider = find_provider(&settings, &route.provider)?;
    let convo = build_convo(&app, &settings, &messages);

    // M4:落地這輪的使用者訊息(回覆在 emit_done 落地)。
    // 主動關心/提醒的合成指令 persist=false,不落 DB。
    if persist {
        if let Some(last) = messages.last() {
            if last.role == "user" {
                crate::memory::log_message(&app, "user", &last.content);
            }
        }
    }

    let mut started = false;
    match run_agent_loop(&app, &provider, &route.model, &settings, &request_id, convo.clone(), persist, &mut started).await {
        Ok(()) => Ok(()),
        Err(err) => {
            // 尚未輸出任何 token、且失敗的不是本地降級目標 → 自動降級
            let can_fallback = settings.fallback_to_local
                && !started
                && provider.id != settings.fallback.provider;
            if can_fallback {
                let fb: TaskRoute = settings.fallback.clone();
                if let Ok(fb_provider) = find_provider(&settings, &fb.provider) {
                    let _ = app.emit(
                        "chat-fallback",
                        json!({
                            "requestId": request_id,
                            "from": provider.name,
                            "to": fb_provider.name,
                            "reason": err,
                        }),
                    );
                    let mut fb_started = false;
                    return match run_agent_loop(&app, &fb_provider, &fb.model, &settings, &request_id, convo, persist, &mut fb_started).await {
                        Ok(()) => Ok(()),
                        Err(e2) => {
                            emit_error(&app, &request_id, &e2);
                            Err(e2)
                        }
                    };
                }
            }
            emit_error(&app, &request_id, &err);
            Err(err)
        }
    }
}

/// 組初始訊息:system(人設+情緒規則+工具提示+長期記憶)+ 截斷後的歷史
fn build_convo(app: &AppHandle, settings: &Settings, messages: &[ChatMessage]) -> Vec<Value> {
    let keep = settings.context_turns.max(1) * 2;
    let history = if messages.len() > keep {
        &messages[messages.len() - keep..]
    } else {
        messages
    };
    let memories = crate::memory::prompt_section(app);
    let mut convo = vec![json!({
        "role": "system",
        "content": system_prompt(&settings.persona, settings.agent_enabled, &memories),
    })];
    for m in history {
        convo.push(json!({ "role": m.role, "content": m.content }));
    }
    convo
}

/// 看截圖(M5):把螢幕 PNG(base64)+ 提示送給 vision 路由的視覺模型。
/// 走 OpenAI 多模態訊息格式;不用工具、不做降級(本地視覺模型沒有雲端對應)。
pub async fn run_vision(
    app: AppHandle,
    settings: Settings,
    request_id: String,
    image_b64: String,
    prompt: String,
) -> Result<(), String> {
    // 舊設定檔可能沒有 vision 路由 → 退回本地預設
    let route = settings.routing.get("vision").cloned().unwrap_or(TaskRoute {
        provider: "ollama".into(),
        model: "qwen2.5vl:3b".into(),
    });
    let provider = find_provider(&settings, &route.provider)?;

    let convo = vec![
        json!({
            "role": "system",
            "content": format!(
                "你是「{name}」。{persona}\n你正在看使用者的螢幕截圖。\
                 用你的口吻、繁體中文、口語且簡短地回應。\
                 回應開頭先給一個情緒標籤(從 [happy] [sad] [angry] [surprised] [shy] [sleepy] [neutral] 擇一)。",
                name = settings.persona.name,
                persona = settings.persona.system_prompt,
            ),
        }),
        json!({
            "role": "user",
            "content": [
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{image_b64}") } },
            ],
        }),
    ];

    let mut started = false;
    let outcome = stream_once(&app, &provider, &route.model, &request_id, &convo, None, &mut started).await;
    match outcome {
        Ok(StreamOutcome::Done(full)) => {
            emit_done(&app, &request_id, &full, false);
            Ok(())
        }
        // 視覺模型不給工具;真有 tool_calls 也當完成處理
        Ok(StreamOutcome::ToolCalls { content, .. }) => {
            emit_done(&app, &request_id, &content, false);
            Ok(())
        }
        Err(err) => {
            emit_error(&app, &request_id, &err);
            Err(err)
        }
    }
}

fn find_provider(settings: &Settings, id: &str) -> Result<ProviderCfg, String> {
    settings
        .providers
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| format!("找不到 Provider:{id}"))
}

fn emit_error(app: &AppHandle, request_id: &str, message: &str) {
    let _ = app.emit(
        "chat-error",
        json!({ "requestId": request_id, "message": message }),
    );
}

/* ---------------- Agent 迴圈(M3) ---------------- */

/// 串流一次的結果:純文字完成,或模型要求呼叫工具
enum StreamOutcome {
    Done(String),
    ToolCalls { calls: Vec<ToolCall>, content: String },
}

#[derive(Debug, Default, Clone)]
struct ToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// 串流 → 執行工具 → 回填結果 → 再串流,直到模型給出純文字回應。
async fn run_agent_loop(
    app: &AppHandle,
    provider: &ProviderCfg,
    model: &str,
    settings: &Settings,
    request_id: &str,
    mut convo: Vec<Value>,
    persist: bool,
    started: &mut bool,
) -> Result<(), String> {
    let tools = if settings.agent_enabled {
        Some(crate::agent::tool_specs(settings.self_dev_enabled))
    } else {
        None
    };
    for _round in 0..MAX_TOOL_ROUNDS {
        match stream_once(app, provider, model, request_id, &convo, tools.as_ref(), started).await? {
            StreamOutcome::Done(full) => {
                emit_done(app, request_id, &full, persist);
                return Ok(());
            }
            StreamOutcome::ToolCalls { calls, content } => {
                // 已進入工具流程,失敗不再降級(工具可能已有副作用)
                *started = true;
                let tc: Vec<Value> = calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": { "name": c.name, "arguments": c.arguments },
                        })
                    })
                    .collect();
                convo.push(json!({ "role": "assistant", "content": content, "tool_calls": tc }));

                for c in &calls {
                    if take_cancelled(app, request_id) {
                        emit_done(app, request_id, &content, persist);
                        return Ok(());
                    }
                    let _ = app.emit(
                        "chat-tool",
                        json!({
                            "requestId": request_id,
                            "name": c.name,
                            "label": crate::agent::tool_label(&c.name),
                        }),
                    );
                    let args: Value = serde_json::from_str(&c.arguments).unwrap_or(json!({}));
                    let result =
                        crate::agent::execute(app, request_id, &c.name, &args, provider.uses_key)
                            .await;
                    convo.push(json!({
                        "role": "tool",
                        "tool_call_id": c.id,
                        "content": result,
                    }));
                }
            }
        }
    }
    Err("工具呼叫太多輪,我先停下來了".into())
}

/// 對單一 provider 發出串流請求。`started` 會在送出第一個 delta 後設為 true,
/// 供呼叫端判斷是否還能安全降級。
async fn stream_once(
    app: &AppHandle,
    provider: &ProviderCfg,
    model: &str,
    request_id: &str,
    convo: &[Value],
    tools: Option<&Value>,
    started: &mut bool,
) -> Result<StreamOutcome, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;

    let mut body = json!({
        "model": model,
        "messages": convo,
        "stream": true,
    });
    if let Some(t) = tools {
        body["tools"] = t.clone();
    }
    // DeepSeek V4 預設開思考模式(回應前內部推理很久,且思考+工具要回傳
    // reasoning_content 才不會 400)。桌寵要即時感,固定關閉。
    if provider.id == "deepseek" {
        body["thinking"] = json!({ "type": "disabled" });
    }

    let url = format!("{}/chat/completions", provider.base_url.trim_end_matches('/'));
    let mut req = client.post(&url).json(&body);
    if provider.uses_key {
        let key = keys::get_key(&provider.id)
            .ok_or_else(|| format!("{} 尚未設定 API Key", provider.name))?;
        req = req.bearer_auth(key);
    }

    let resp = req.send().await.map_err(|e| format!("連線失敗:{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("{} 回應 HTTP {status}:{}", provider.name, truncate(&body, 200)));
    }

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut full = String::new();
    let mut calls: Vec<ToolCall> = Vec::new();

    'outer: while let Some(chunk) = stream.next().await {
        // 取消:以當下累積內容收尾,前端視同正常完成
        if take_cancelled(app, request_id) {
            return Ok(StreamOutcome::Done(full));
        }
        let chunk = chunk.map_err(|e| format!("串流中斷:{e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload == "[DONE]" {
                break 'outer;
            }
            let Ok(v) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            let delta = &v["choices"][0]["delta"];
            // 注:deepseek-reasoner 另有 delta.reasoning_content(思考過程),桌寵不顯示
            if let Some(text) = delta["content"].as_str() {
                if !text.is_empty() {
                    *started = true;
                    full.push_str(text);
                    let _ = app.emit(
                        "chat-delta",
                        json!({ "requestId": request_id, "delta": text }),
                    );
                }
            }
            if let Some(tcs) = delta["tool_calls"].as_array() {
                accumulate_tool_calls(&mut calls, tcs);
            }
        }
    }

    // 部分伺服器不送 [DONE],串流自然結束也視為完成
    if calls.iter().any(|c| !c.name.is_empty()) {
        calls.retain(|c| !c.name.is_empty());
        Ok(StreamOutcome::ToolCalls { calls, content: full })
    } else {
        Ok(StreamOutcome::Done(full))
    }
}

/// 累積串流中的 tool_calls 片段(OpenAI 以 index 分段傳 id/name/arguments)
fn accumulate_tool_calls(calls: &mut Vec<ToolCall>, fragments: &[Value]) {
    for frag in fragments {
        let idx = frag["index"].as_u64().unwrap_or(0) as usize;
        while calls.len() <= idx {
            calls.push(ToolCall::default());
        }
        let call = &mut calls[idx];
        if let Some(id) = frag["id"].as_str() {
            if !id.is_empty() {
                call.id = id.to_string();
            }
        }
        if let Some(name) = frag["function"]["name"].as_str() {
            if !name.is_empty() {
                call.name = name.to_string();
            }
        }
        let args = &frag["function"]["arguments"];
        if let Some(s) = args.as_str() {
            call.arguments.push_str(s);
        } else if args.is_object() {
            // 某些實作直接給整包 JSON 物件而非字串片段
            call.arguments = args.to_string();
        }
    }
}

/// 檢查並消耗取消旗標(remove 回傳 true 表示曾被要求取消)
fn take_cancelled(app: &AppHandle, request_id: &str) -> bool {
    app.state::<CancelState>().0.lock().unwrap().remove(request_id)
}

fn emit_done(app: &AppHandle, request_id: &str, full: &str, persist: bool) {
    // M4:回覆落地對話紀錄(去掉開頭情緒標籤);主動關心不落 DB
    if persist {
        let clean = full
            .trim_start()
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .map(|(_, text)| text.trim())
            .unwrap_or(full.trim());
        crate::memory::log_message(app, "assistant", clean);
    }
    let _ = app.emit(
        "chat-done",
        json!({ "requestId": request_id, "content": full }),
    );
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}
