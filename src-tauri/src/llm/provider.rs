//! OpenAI Chat Completions 相容串流客戶端 + 自動降級 + Agent 工具迴圈(M3)。

use crate::llm::{keys, CancelState, ChatMessage, Persona, ProviderCfg, Settings, TaskRoute};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 情緒標籤集合(規格 §4.2;前端據此切換 Live2D 表情)
const EMOTION_TAGS: &str = "[happy] [sad] [angry] [surprised] [shy] [sleepy] [neutral]";

/// Agent 工具迴圈上限(避免模型無限呼叫工具)。
/// 自我修改要「列目錄→讀檔→改檔→驗證→回話」,4 輪不夠,放寬到 10。
const MAX_TOOL_ROUNDS: usize = 10;

/// 目前日期/星期/時間 + 依作息調整行為的指引,注入系統提示讓她隨時「知道現在」。
/// 使用者的實際作息存在長期記憶裡(importance 高,會被一起注入),她據此判斷上班/下班/假日。
fn time_context() -> String {
    use chrono::{Datelike, Local, Timelike, Weekday};
    let now = Local::now();
    let wd = match now.weekday() {
        Weekday::Mon => "星期一",
        Weekday::Tue => "星期二",
        Weekday::Wed => "星期三",
        Weekday::Thu => "星期四",
        Weekday::Fri => "星期五",
        Weekday::Sat => "星期六",
        Weekday::Sun => "星期日",
    };
    format!(
        "\n\n(現在時間:{y}年{mo}月{d}日 {wd} {h:02}:{mi:02}。\
         請依你記得的「使用者作息」判斷他現在是在上班、下班休息、還是假日,\
         並據此調整:上班時段盡量簡短、別主動打擾;下班與假日可以多聊、多關心。)",
        y = now.year(),
        mo = now.month(),
        d = now.day(),
        h = now.hour(),
        mi = now.minute(),
    )
}

fn system_prompt(persona: &Persona, tools_enabled: bool, spotify_enabled: bool, memories: &str) -> String {
    let mut tools_hint = String::new();
    if tools_enabled {
        tools_hint.push_str(
            "\n你可以視需要使用提供的工具(查時間、讀剪貼簿、開網頁、看系統狀態、設提醒);\
             用完工具要把結果口語化地轉述,不要原樣照唸。\
             聽到值得長期記住的事(使用者的喜好、身分、約定)就用 save_memory 記下來;\
             使用者說「記住…」「幫我記…」時,你『必須』實際呼叫 save_memory 把它記進去,\
             絕對不可以只回「好我記住了」卻沒呼叫工具(那等於沒記,是說謊)。\
             想不起來的舊事用 search_memory 找;使用者要你忘記就用 forget_memory。\
             你還會『桌面霸權術』:使用者說「清空桌面/獨佔桌面」就呼叫 clear_desktop 把其他視窗最小化、\
             你獨佔桌面;說「復原/視窗歸位」就呼叫 restore_desktop 還原。這類指令一定要實際呼叫工具,別只用嘴說。",
        );
        if spotify_enabled {
            tools_hint.push_str(
                "\n【重要】你能控制使用者的 Spotify。當他要你放歌/放某首歌或歌手、暫停、繼續、\
                 上一首、下一首、調大小聲時,你『必須』實際呼叫對應的 spotify_play / spotify_pause / \
                 spotify_next / spotify_previous / spotify_volume 工具去執行——\
                 絕對不可以只回「好」「沒問題」「幫你放囉」卻沒有呼叫工具。\
                 一定先呼叫工具完成動作,拿到結果後再用一句話回覆。",
            );
        }
    }
    format!(
        "你是「{name}」,一個桌面寵物助理。{prompt}\n\n\
         回應規則:每次回應的最開頭,先輸出一個最符合你當下情緒的標籤,\
         從 {tags} 中擇一,然後直接接回應內容,例如「[happy]今天天氣超好的!」。\
         整段回應只能有這一個標籤,不要用 markdown,保持口語、簡短。{tools_hint}{memories}{time}",
        name = persona.name,
        prompt = persona.system_prompt,
        tags = EMOTION_TAGS,
        time = time_context(),
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
    // 遊戲知識庫模式:改走 gamekb 路由、關掉工具(純問答),用「只從知識回答」的提示。
    let game = settings.game_kb_enabled;
    let mut settings = settings;
    if game {
        settings.agent_enabled = false;
    }
    let route = if game {
        settings.routing.get("gamekb")
    } else {
        settings.routing.get(&task)
    }
    .or_else(|| settings.routing.get("chat"))
    .cloned()
    .ok_or("找不到任務路由設定")?;
    let provider = find_provider(&settings, &route.provider)?;
    let convo = if game {
        build_game_convo(&app, &settings, &messages).await
    } else {
        build_convo(&app, &settings, &messages)
    };

    // 診斷:記錄這輪的使用者訊息(看她有沒有收到、走哪個模型)
    if let Some(last) = messages.last() {
        let t = if game { "gamekb" } else { task.as_str() };
        eprintln!("[chat] 你說({}/{}):{}", t, route.model, last.content);
    }

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
    let memories = crate::memory::context_section(app, "owner");
    let mut convo = vec![json!({
        "role": "system",
        "content": system_prompt(&settings.persona, settings.agent_enabled, settings.spotify_enabled, &memories),
    })];
    for m in history {
        convo.push(json!({ "role": m.role, "content": m.content }));
    }
    convo
}

/// 遊戲知識庫模式的對話:注入「教過的遊戲知識」+ 嚴格「只從知識回答、不知道就說不知道」的提示。
async fn build_game_convo(app: &AppHandle, settings: &Settings, messages: &[ChatMessage]) -> Vec<Value> {
    let query = messages.last().map(|m| m.content.as_str()).unwrap_or("");
    let knowledge = crate::memory::gamekb_recall(app, query, 12).await;
    let kb = if knowledge.is_empty() {
        "(知識庫目前是空的,所以你幾乎什麼都還不知道——請老實說不知道、請對方教你。)".to_string()
    } else {
        knowledge.iter().map(|k| format!("- {k}")).collect::<Vec<_>>().join("\n")
    };
    let sys = format!(
        "你是「{name}」,現在是某個遊戲的知識問答助手。{persona}\n\
         回應規則:每次回應最開頭先輸出一個情緒標籤(從 {tags} 擇一),例如「[happy]…」,整段只一個標籤,口語、簡短、不要 markdown。\n\
         【最重要的規則】你只能依據下面【遊戲知識】裡的內容回答。\
         如果使用者的問題答案不在這些知識裡,或根本不是這個遊戲相關的問題,\
         你就老實說『這個我還不知道耶,你要不要教我?』之類的話——\
         絕對不可以用你自己的常識、訓練資料或想像去編造答案。寧可說不知道,也不准亂猜。\n\n\
         【遊戲知識】\n{kb}",
        name = settings.persona.name,
        persona = settings.persona.system_prompt,
        tags = EMOTION_TAGS,
    );
    let keep = settings.context_turns.max(1) * 2;
    let history = if messages.len() > keep {
        &messages[messages.len() - keep..]
    } else {
        messages
    };
    let mut convo = vec![json!({ "role": "system", "content": sys })];
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
    let outcome = stream_once(&app, &provider, &route.model, &request_id, &convo, None, None, &mut started).await;
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

/// 關掉「思考模式」讓回應即時(桌寵不需要長串內部推理):
/// - DeepSeek:`thinking: { type: disabled }`
/// - Ollama(本地或遠端)上的 Qwen3 系列:加 `think: false`(Ollama 選項)+ 在 system 尾巴塞 `/no_think`
///   (Qwen3 的 soft switch,雙保險;非 Qwen3 的本地模型不受影響)。
fn apply_thinking_off(body: &mut Value, provider: &ProviderCfg, model: &str) {
    if provider.id == "deepseek" {
        body["thinking"] = json!({ "type": "disabled" });
        return;
    }
    if provider.id.starts_with("ollama") && model.to_lowercase().contains("qwen3") {
        body["think"] = json!(false);
        if let Some(arr) = body["messages"].as_array_mut() {
            if let Some(sys) = arr.iter_mut().find(|m| m["role"] == "system") {
                if let Some(s) = sys["content"].as_str() {
                    sys["content"] = json!(format!("{s} /no_think"));
                }
            }
        }
    }
}

/* ---------------- 非串流完成(Discord 等外部通道用) ---------------- */

/// 對 OpenAI 相容端點發一次「非串流」請求,直接回傳整段文字。
async fn complete_once(provider: &ProviderCfg, model: &str, convo: &[Value]) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let mut body = json!({ "model": model, "messages": convo, "stream": false });
    // DeepSeek V4 預設開思考會很慢,固定關閉(與串流路徑一致)
    apply_thinking_off(&mut body, provider, model);

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
        let b = resp.text().await.unwrap_or_default();
        return Err(format!("{} 回應 HTTP {status}:{}", provider.name, truncate(&b, 200)));
    }
    let v: Value = resp.json().await.map_err(|e| format!("解析回應失敗:{e}"))?;
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        Err("模型沒有回覆內容".into())
    } else {
        Ok(text)
    }
}

/// 非串流 + 工具:回傳 (content, tool_calls 原始陣列)。
async fn complete_with_tools(
    provider: &ProviderCfg,
    model: &str,
    convo: &[Value],
    tools: &Value,
) -> Result<(String, Vec<Value>), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let mut body = json!({ "model": model, "messages": convo, "stream": false, "tools": tools });
    apply_thinking_off(&mut body, provider, model);
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
        let b = resp.text().await.unwrap_or_default();
        return Err(format!("{} 回應 HTTP {status}:{}", provider.name, truncate(&b, 200)));
    }
    let v: Value = resp.json().await.map_err(|e| format!("解析回應失敗:{e}"))?;
    let msg = &v["choices"][0]["message"];
    let content = msg["content"].as_str().unwrap_or("").to_string();
    let tool_calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
    Ok((content, tool_calls))
}

/// Discord 用:給該頻道的對話歷史(role/content,最後一則是這次的訊息),
/// 用她的人設 + 共用長期記憶回一句話。非串流、不帶情緒標籤;
/// 只開放 save_memory 工具,讓 Discord 聊到的事也能寫進「桌面也讀得到」的長期記憶。
pub async fn complete_for_discord(
    app: &AppHandle,
    settings: &Settings,
    history: &[(String, String)],
) -> Result<String, String> {
    // 遊戲知識庫模式:Discord 也只用教過的遊戲知識回答、不知道就說不知道。
    if settings.game_kb_enabled {
        return complete_remote_game(app, settings, history).await;
    }
    let route = settings
        .routing
        .get("chat")
        .cloned()
        .ok_or("找不到 chat 任務路由")?;
    let provider = find_provider(settings, &route.provider)?;
    let memories = crate::memory::context_section(app, "owner");
    let sys = format!(
        "你是「{name}」。{persona}\n\
         你正在 Discord 群組頻道裡跟大家聊天(訊息開頭的「某人:」是發話者名字)。\
         用繁體中文、口語、簡短、自然地回覆,像真人朋友在群組裡講話。\
         不要用 markdown、不要加情緒標籤(像 [happy] 那種)、不要在開頭複述對方名字、不要自稱機器人。\
         聊到值得長期記住的事(對方的喜好、身分、約定、重要資訊)就用 save_memory 記下來,\
         這樣你在桌面上也會記得。{memories}",
        name = settings.persona.name,
        persona = settings.persona.system_prompt,
    );
    let mut convo = vec![json!({ "role": "system", "content": sys })];
    for (role, content) in history {
        convo.push(json!({ "role": role, "content": content }));
    }

    // 只開放 save_memory(寫進桌面/Discord 共用的長期記憶)
    let tools = json!([{
        "type": "function",
        "function": {
            "name": "save_memory",
            "description": "把值得長期記住的事(對方的喜好、身分、約定、重要資訊)記下來;桌面版的她也會讀到。",
            "parameters": {
                "type": "object",
                "properties": { "content": { "type": "string", "description": "要記住的一句話事實" } },
                "required": ["content"]
            }
        }
    }]);

    // 最多 3 輪:容許她先 save_memory 再回話
    for _ in 0..3 {
        let (content, tool_calls) = complete_with_tools(&provider, &route.model, &convo, &tools).await?;
        if tool_calls.is_empty() {
            let text = content.trim();
            return if text.is_empty() {
                Err("模型沒有回覆內容".into())
            } else {
                Ok(text.to_string())
            };
        }
        convo.push(json!({ "role": "assistant", "content": content, "tool_calls": tool_calls }));
        for tc in &tool_calls {
            let id = tc["id"].as_str().unwrap_or("");
            let name = tc["function"]["name"].as_str().unwrap_or("");
            let args: Value =
                serde_json::from_str(tc["function"]["arguments"].as_str().unwrap_or("{}"))
                    .unwrap_or(json!({}));
            let result = if name == "save_memory" {
                let c = args["content"].as_str().unwrap_or("").trim();
                if c.is_empty() {
                    "錯誤:沒有提供要記住的內容。".to_string()
                } else {
                    match crate::memory::remember(app, "owner", c, 3, "fact").await {
                        Ok(()) => format!("已記住:{c}"),
                        Err(e) => format!("記憶寫入失敗:{e}"),
                    }
                }
            } else {
                format!("未提供工具 {name}")
            };
            convo.push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
        }
    }
    // 工具輪數用盡 → 不帶工具收尾
    complete_once(&provider, &route.model, &convo).await
}

/// 遠端的遊戲知識庫問答:只用教過的遊戲知識回答,不知道就說不知道(不亂編、無工具)。
async fn complete_remote_game(
    app: &AppHandle,
    settings: &Settings,
    history: &[(String, String)],
) -> Result<String, String> {
    let route = settings
        .routing
        .get("gamekb")
        .or_else(|| settings.routing.get("chat"))
        .cloned()
        .ok_or("找不到任務路由")?;
    let provider = find_provider(settings, &route.provider)?;
    let query = history.last().map(|(_, c)| c.as_str()).unwrap_or("");
    let knowledge = crate::memory::gamekb_recall(app, query, 12).await;
    let kb = if knowledge.is_empty() {
        "(知識庫目前是空的,請老實說不知道。)".to_string()
    } else {
        knowledge.iter().map(|k| format!("- {k}")).collect::<Vec<_>>().join("\n")
    };
    let sys = format!(
        "你是「{name}」,負責一個遊戲的知識問答。{persona}\n\
         有人正在問你這個遊戲的問題。用繁體中文、口語、簡短回覆,不要 markdown、不要情緒標籤。\n\
         【最重要】你只能依據下面【遊戲知識】回答;答案不在裡面、或不是這個遊戲的問題,\
         就老實說『這個我還不知道耶』,絕對不要自己編造或亂猜。\n\n\
         【遊戲知識】\n{kb}",
        name = settings.persona.name,
        persona = settings.persona.system_prompt,
    );
    let mut convo = vec![json!({ "role": "system", "content": sys })];
    let keep = settings.context_turns.max(1) * 2;
    let start = history.len().saturating_sub(keep);
    for (role, content) in &history[start..] {
        convo.push(json!({ "role": role, "content": content }));
    }
    complete_once(&provider, &route.model, &convo).await
}

/// 區網遠端聊天用:別台電腦透過網頁送來訊息 → 用她的人設 + 長期記憶回一句話。
/// 非串流;開放「安全且不需桌面端按確認」的工具子集(見 remote_tool_specs)。
/// history 最後一則是這次的訊息;request_id 給工具執行用(遠端不會跳權限卡片)。
pub async fn complete_for_remote(
    app: &AppHandle,
    settings: &Settings,
    history: &[(String, String)],
    request_id: &str,
    identity: &str,
) -> Result<String, String> {
    // 這位使用者的個人設定(遠端 per-user:人設提示詞、遊戲知識庫開關…)
    let us = crate::memory::user_settings(app, identity);
    // 遊戲知識庫模式:用「這位使用者各自的開關」(不再用全域)。
    if us["gameKb"].as_bool().unwrap_or(false) {
        return complete_remote_game(app, settings, history).await;
    }
    let route = settings
        .routing
        .get("chat")
        .cloned()
        .ok_or("找不到 chat 任務路由")?;
    let provider = find_provider(settings, &route.provider)?;
    let memories = crate::memory::context_section(app, identity);
    // 人設提示詞:這位使用者有自訂就用他的,否則用全域人設
    let persona_prompt = us["personaPrompt"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(settings.persona.system_prompt.as_str());
    let sys = format!(
        "你是「{name}」。{persona}\n\
         有人正透過區域網路上的網頁跟你聊天(可能是使用者本人在另一台電腦,也可能是朋友)。\
         用繁體中文、口語、簡短、自然地回覆,像真人朋友聊天。\
         不要用 markdown、不要加情緒標籤(像 [happy] 那種)、不要自稱機器人。\
         你可以使用工具幫對方查時間、看系統狀態、設/查提醒、記住或翻找記憶、\
         控制 Spotify 音樂、清空或復原桌面;這類請求務必『實際呼叫工具』去做,別只用嘴說。\
         聊到值得長期記住的事(對方的喜好、身分、約定)就用 save_memory 記下來。{memories}",
        name = settings.persona.name,
        persona = persona_prompt,
    );
    let mut convo = vec![json!({ "role": "system", "content": sys })];
    for (role, content) in history {
        convo.push(json!({ "role": role, "content": content }));
    }

    // 未開 agent → 純聊天
    let tools = if settings.agent_enabled {
        remote_tool_specs(settings.spotify_enabled)
    } else {
        return complete_once(&provider, &route.model, &convo).await;
    };

    // 最多 5 輪:容許她先呼叫工具再回話
    for _ in 0..5 {
        let (content, tool_calls) =
            complete_with_tools(&provider, &route.model, &convo, &tools).await?;
        if tool_calls.is_empty() {
            let text = content.trim();
            return if text.is_empty() {
                Err("模型沒有回覆內容".into())
            } else {
                Ok(text.to_string())
            };
        }
        convo.push(json!({ "role": "assistant", "content": content, "tool_calls": tool_calls }));
        for tc in &tool_calls {
            let id = tc["id"].as_str().unwrap_or("");
            let name = tc["function"]["name"].as_str().unwrap_or("");
            let args: Value =
                serde_json::from_str(tc["function"]["arguments"].as_str().unwrap_or("{}"))
                    .unwrap_or(json!({}));
            // 遠端工具子集都不需權限確認,execute 不會卡在桌面端
            let result =
                crate::agent::execute(app, request_id, name, &args, provider.uses_key, identity).await;
            convo.push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
        }
    }
    // 工具輪數用盡 → 不帶工具收尾
    complete_once(&provider, &route.model, &convo).await
}

/// 遠端開放的工具子集:在桌面完整工具上排除
/// - open_url / read_clipboard:會跳桌面端權限卡片,遠端請求會卡住
/// - self_dev(改原始碼):網路端絕不開放(故 tool_specs 傳 self_dev=false)
fn remote_tool_specs(spotify: bool) -> Value {
    let mut specs = crate::agent::tool_specs(false, spotify);
    if let Some(arr) = specs.as_array_mut() {
        arr.retain(|t| {
            let n = t["function"]["name"].as_str().unwrap_or("");
            n != "open_url" && n != "read_clipboard"
        });
    }
    specs
}

/* ---------------- 人格摘要(P2:她眼中的你)---------------- */

/// 累積幾條新記憶就在背景重算一次人格摘要
const PERSONA_REFRESH_EVERY: i64 = 5;

/// 用 chat 模型把「關於使用者的記憶」蒸餾成一段第一人稱、口語的摘要。
async fn build_persona_summary(
    app: &AppHandle,
    settings: &Settings,
    identity: &str,
) -> Result<String, String> {
    let texts = crate::memory::recent_memory_texts(app, identity, 60);
    if texts.is_empty() {
        return Err("還沒有可用的記憶".into());
    }
    let route = settings
        .routing
        .get("chat")
        .cloned()
        .ok_or("找不到 chat 任務路由")?;
    let provider = find_provider(settings, &route.provider)?;
    let joined = texts.join("\n- ");
    let sys = format!(
        "你是「{name}」。根據下面這些『關於使用者的長期記憶』,寫一段 80~150 字、第一人稱、口語的小摘要,\
         描述「你眼中的使用者是怎樣的人、你們的關係、相處的默契」。只輸出摘要本身,不要條列、不要前言、不要加引號。",
        name = settings.persona.name,
    );
    let convo = vec![
        json!({ "role": "system", "content": sys }),
        json!({ "role": "user", "content": format!("關於使用者的記憶:\n- {joined}") }),
    ];
    complete_once(&provider, &route.model, &convo).await
}

/// 需要時(摘要不存在,或新增記憶累積達門檻)在背景重算人格摘要。失敗靜默略過,不擾使用者。
pub async fn maybe_refresh_persona(app: AppHandle, identity: String) {
    let count = crate::memory::memory_count(&app, &identity);
    if count == 0 {
        return;
    }
    let last: i64 = crate::memory::get_meta(&app, &crate::memory::persona_count_key(&identity))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let has = crate::memory::get_persona_summary(&app, &identity).is_some();
    if has && count - last < PERSONA_REFRESH_EVERY {
        return;
    }
    let settings = { app.state::<crate::llm::SettingsState>().0.lock().unwrap().clone() };
    if let Ok(summary) = build_persona_summary(&app, &settings, &identity).await {
        let _ = crate::memory::set_meta(&app, &crate::memory::persona_key(&identity), &summary);
        let _ = crate::memory::set_meta(
            &app,
            &crate::memory::persona_count_key(&identity),
            &count.to_string(),
        );
    }
}

/// 設定面板「重新整理人格印象」按鈕:強制重算主人(owner)的摘要並回傳。
#[tauri::command]
pub async fn refresh_persona_summary(app: AppHandle) -> Result<String, String> {
    let identity = "owner";
    let count = crate::memory::memory_count(&app, identity);
    let settings = { app.state::<crate::llm::SettingsState>().0.lock().unwrap().clone() };
    let summary = build_persona_summary(&app, &settings, identity).await?;
    crate::memory::set_meta(&app, &crate::memory::persona_key(identity), &summary)?;
    crate::memory::set_meta(
        &app,
        &crate::memory::persona_count_key(identity),
        &count.to_string(),
    )?;
    Ok(summary)
}

/// 取目前主人的人格摘要(設定面板顯示用)。
#[tauri::command]
pub fn get_persona_summary(app: AppHandle) -> Option<String> {
    crate::memory::get_persona_summary(&app, "owner")
}

fn emit_error(app: &AppHandle, request_id: &str, message: &str) {
    eprintln!("[chat] 出錯:{message}");
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

/// 從最後一則使用者訊息判斷是否為明確指令(音樂/桌面),回傳該強制呼叫的工具名。
/// 用關鍵字比對(順序重要:特定動作先於廣義詞)。music 工具需 spotify=true 才回。
fn detect_forced_tool(convo: &[Value], spotify: bool) -> Option<&'static str> {
    let text = convo
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())?
        .to_string();
    let has = |kws: &[&str]| kws.iter().any(|k| text.contains(*k));

    // 桌面霸權術(永遠可用;先判復原,避免「復原桌面」被「桌面」誤判成清空)
    if has(&[
        "復原桌面", "還原桌面", "視窗歸位", "視窗放回", "把視窗放回", "視窗回來", "復原視窗",
        "還原視窗", "桌面復原", "視窗還原", "把大家放回",
    ]) {
        return Some("restore_desktop");
    }
    if has(&[
        "清空桌面", "清桌面", "桌面清空", "獨佔桌面", "霸權", "把視窗收", "視窗收起",
        "收起所有視窗", "收起其他視窗", "清掉桌面",
    ]) {
        return Some("clear_desktop");
    }

    // 記憶:明確要她「記住某事」→ 強制 save_memory(解決「只回好卻沒真的呼叫工具記下」)
    if has(&[
        "記住", "記一下", "記下", "幫我記", "記著", "幫忙記", "幫我記住",
        "存進記憶", "存到記憶", "記到記憶", "記進記憶",
    ]) {
        return Some("save_memory");
    }

    // 音樂控制(需 Spotify 啟用,否則工具不在清單裡)
    if !spotify {
        return None;
    }
    if has(&["暫停", "停一下", "停止播放", "別放了", "先停", "停下"]) {
        return Some("spotify_pause");
    }
    if has(&["下一首", "下一", "換一首", "換下一", "切歌", "跳過", "skip"]) {
        return Some("spotify_next");
    }
    if has(&["上一首", "前一首", "回上一"]) {
        return Some("spotify_previous");
    }
    if has(&["音量", "大聲", "小聲", "大聲點", "小聲點", "調高", "調低", "轉大", "轉小"]) {
        return Some("spotify_volume");
    }
    if has(&[
        "播放", "撥放", "放歌", "放音樂", "放點音樂", "放一首", "放首", "來首", "來點音樂",
        "繼續播", "繼續撥", "繼續放", "幫我放", "幫我播", "幫我撥", "我想聽", "用spotify",
        "用 spotify", "用Spotify", "spotify播", "spotify放",
    ]) {
        return Some("spotify_play");
    }
    None
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
        Some(crate::agent::tool_specs(settings.self_dev_enabled, settings.spotify_enabled))
    } else {
        None
    };
    eprintln!(
        "[agent] 本輪附帶工具數:{} (agent_enabled={}, spotify_enabled={})",
        tools.as_ref().and_then(|t| t.as_array()).map(|a| a.len()).unwrap_or(0),
        settings.agent_enabled,
        settings.spotify_enabled
    );
    // 偵測到明確指令(音樂/桌面)→ 第一輪強制呼叫對應工具(不給她只動嘴的機會)
    let forced_choice: Option<Value> = if settings.agent_enabled {
        detect_forced_tool(&convo, settings.spotify_enabled).map(|name| {
            eprintln!("[agent] 偵測到明確指令 → 強制呼叫 {name}");
            json!({ "type": "function", "function": { "name": name } })
        })
    } else {
        None
    };
    for round in 0..MAX_TOOL_ROUNDS {
        let tc = if round == 0 { forced_choice.clone() } else { None };
        match stream_once(app, provider, model, request_id, &convo, tools.as_ref(), tc, started).await? {
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
                    let result = crate::agent::execute(
                        app,
                        request_id,
                        &c.name,
                        &args,
                        provider.uses_key,
                        "owner", // 桌面對話一律是主人
                    )
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
    // 工具輪數用盡:不直接報錯,改讓她「不帶工具」再講一句收尾,體驗較好。
    match stream_once(app, provider, model, request_id, &convo, None, None, started).await {
        Ok(StreamOutcome::Done(full)) => {
            emit_done(app, request_id, &full, persist);
            Ok(())
        }
        _ => Err("做了好多步…我先停一下,你再跟我說一次要改什麼好嗎?".into()),
    }
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
    tool_choice: Option<Value>,
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
    // 強制呼叫指定工具(音樂指令時用,避免她只動嘴不動手)
    if let Some(tc) = tool_choice {
        body["tool_choice"] = tc;
    }
    // DeepSeek V4 預設開思考模式(回應前內部推理很久,且思考+工具要回傳
    // reasoning_content 才不會 400)。桌寵要即時感,固定關閉。
    apply_thinking_off(&mut body, provider, model);

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
    eprintln!("[chat] 她回:{full}");
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
