# Desktop Pet AI — Live2D 桌面寵物 AI 助理

Windows 桌面常駐的 Live2D 虛擬人助理,全本地運算優先。本倉庫目前完成 **M0 骨架 + M1 對話核心 + M2 語音(TTS/STT/對嘴)+ M3 前半(Agent 工具呼叫)+ M4 長期記憶**。

## 目前進度

- ✅ Tauri v2 + Vue 3 + Vite 專案骨架
- ✅ 透明無邊框、always-on-top、無陰影視窗
- ✅ 拖曳角色移動 + 位置記憶(重啟還原)
- ✅ 系統托盤:顯示/隱藏、點擊穿透、靜音、設定(占位)、結束
- ✅ 全域快捷鍵 `Ctrl+Shift+A` 喚出/收合對話輸入框(M0 為占位回應)
- ✅ Live2D 模型載入與渲染(pixi.js v7 + pixi-live2d-display-lipsyncpatch)
- ✅ 互動:點頭部摸頭表情、點身體隨機台詞/動作、視線追蹤、閒置小動作
- ✅ M1:Provider 抽象層(OpenAI 相容)— Ollama + DeepSeek、任務路由(閒聊/寫程式/推理)
- ✅ M1:SSE 串流輸出、雲端失敗自動降級本地(角色口頭告知)
- ✅ M1:人設系統、情緒標籤 `[happy]` 等 → Live2D 表情/動作(可在 active.json 自訂映射)
- ✅ M1:設定面板(人設、路由、降級開關、保留輪數)、DeepSeek Key 存 Windows 認證管理員
- ✅ M1:啟動健康檢查(Ollama 未運行時給安裝指引)
- ✅ M1.5:取消生成(思考中送新訊息會打斷舊回應)、智慧點擊穿透(游標不在角色上自動穿透)
- ✅ M2 前半:TTS 朗讀 AI 回覆(WebView2 內建 speechSynthesis,免 sidecar)+ 正弦口型同步;設定面板可選語音/語速/音量
- ✅ M2 後半:Piper 高品質 TTS(`model.speak()` 真實波形對嘴)+ Whisper.cpp 語音輸入(`Ctrl+Shift+S`);sidecar 缺席時自動退回系統語音、`scripts\setup-speech.ps1` 一鍵安裝
- ✅ M2.5:Edge 神經網路語音(msedge-tts,免金鑰、曉伊/曉曉/曉臻等甜美聲線、MP3 對嘴);引擎鏈 Edge → Piper → 系統逐級退回,設定面板可選聲線
- ✅ M3 前半:Agent 工具呼叫(OpenAI function calling、串流 tool_calls 解析、多輪工具迴圈)+ 權限分級(唯讀自動;開網頁/雲端讀剪貼簿先跳確認卡片,60 秒未回應視同拒絕)。內建工具:查時間、讀剪貼簿、開網頁、看系統狀態
- ✅ M4 長期記憶:SQLite(`%APPDATA%\com.desktoppet.ai\memory.db`)。她用 save_memory / search_memory / forget_memory 工具自主記憶,最近 30 條注入 system prompt;對話紀錄落地、重啟自動接上;設定面板可一鍵清除
- ✅ M4.5 主動行為:提醒(set/list/cancel_reminder 工具 + scheduler 每 20 秒檢查,到期主動跳視窗+朗讀;關閉期間錯過的開機補發)+ 閒置主動找話題(可在設定開關/調閒置分鐘數);主動發話走 persist=false 不污染對話紀錄
- ⬜ M3 後半:寫入類工具(寫檔、執行指令)+ 沙箱
- ⬜ M4.6:記憶向量檢索(語意搜尋舊記憶)

## 環境需求

1. **Node.js** 20+
2. **Rust**(stable,經 [rustup](https://rustup.rs) 安裝)
3. **Visual Studio Build Tools**(含「使用 C++ 的桌面開發」工作負載)
4. **WebView2 Runtime**(Windows 11 內建)

## 安裝與啟動

**懶人包**:`powershell -ExecutionPolicy Bypass -File scripts\setup.ps1` 會自動安裝 Node / Rust / VS Build Tools、執行 npm install,並引導下載 Cubism Core。

手動安裝:

```powershell
cd desktop-pet-ai
npm install

# 1) 放置 Cubism Core(必要,授權因素不可隨倉庫散布)
#    從 https://www.live2d.com/sdk/download/web/ 下載 SDK,
#    將 Core/live2dcubismcore.min.js 複製到 public/vendor/

# 2) 放置 Live2D 模型(必要)
#    將模型資料夾放入 public/models/<角色名>/,
#    並建立 public/models/active.json(參考 active.example.json)

# 3) 對話腦袋(二擇一或都要)
#    本地:winget install Ollama.Ollama && ollama pull qwen2.5:7b
#    雲端:托盤 → 設定 → 填入 DeepSeek API Key

# 4) 語音(選用):Piper TTS + Whisper 語音輸入
#    powershell -ExecutionPolicy Bypass -File scripts\setup-speech.ps1
#    沒裝也能用:朗讀自動退回 Windows 系統語音,只有語音輸入需要 Whisper

# 開發模式
npm run tauri dev

# 建置 Windows 安裝包(NSIS,輸出於 src-tauri/target/release/bundle/nsis/)
npm run tauri build
```

> 未放置 Cubism Core 或模型時,App 仍可啟動,會顯示引導畫面。

### 新增 / 切換角色

把模型資料夾放進 `public/models/<角色名>/`,在 `public/models/characters.json` 的 `characters` 陣列加一筆(`id`、`name`、`path` 指到 `.model3.json`、可選 `scale`/`idleMinutes`/`emotions`),即可在「設定 → 角色外觀」即時切換(選擇記在 localStorage)。`emotions` 把情緒標籤(happy/sad/angry/surprised/shy/sleepy)映射到該模型的 expression 名稱或 `motion:群組名`。
若模型的 `model3.json` 沒宣告 `Expressions`/`Motions` 或 `LipSync`/`EyeBlink` 群組是空的(常見於 VTube Studio 匯出),需補上才有表情、動作、對嘴與眨眼(可參考 `public/models/icegirl/IceGirl.model3.json`)。

## 操作說明

| 操作 | 行為 |
|---|---|
| 按住角色拖曳 | 移動視窗(位置自動記憶) |
| 點擊角色頭部 / 懸停頭部 | 摸頭表情 |
| 點擊角色身體 | 隨機台詞 + 動作 |
| `Ctrl+Shift+A` | 喚出/收合對話輸入框(Enter 送出,串流顯示+情緒表情+朗讀) |
| `Ctrl+Shift+S` | 開始/結束語音輸入(需 Whisper,見 setup-speech.ps1;最長 60 秒自動結束) |
| 思考中再送一句 | 打斷舊回應,直接回答新訊息 |
| 「○分鐘後提醒我…」/「提醒我幾點…」 | 她設提醒,到時主動跳出來講(關 App 期間錯過的會在開機補發) |
| 閒置太久 | 她會主動找你聊天(可在設定關閉或調整分鐘數) |
| 托盤 → 設定 → 角色外觀 | 切換 Live2D 角色(即時切換,記住選擇) |
| 托盤 → 設定 | 角色外觀、人設、模型路由、DeepSeek Key、降級開關、語音(TTS)、主動行為、記憶 |
| 托盤 → 點擊穿透(整個視窗) | 滑鼠事件穿透到下層視窗(再點一次恢復) |
| 托盤 → 智慧穿透 | 游標在角色/UI 上才攔截滑鼠,其餘區域穿透到下層 |
| 托盤 → 靜音 | 關閉台詞泡泡碎念與 TTS 朗讀 |
| 托盤 → 結束 | 真正退出(點視窗關閉只會隱藏到托盤) |

## 專案結構

```
desktop-pet-ai/
├── src-tauri/               # Rust 核心
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs           # Builder 組裝;後續模組規劃見註解
│   │   ├── window.rs        # 視窗/托盤/快捷鍵/位置記憶/點擊穿透
│   │   ├── llm/             # Provider 抽象、SSE 串流、降級、金鑰(M1)+ Agent 迴圈(M3)
│   │   ├── speech/          # Piper TTS / Whisper STT sidecar 管理(M2)
│   │   ├── agent/           # 工具註冊/執行、權限確認(M3)
│   │   ├── memory/          # SQLite 長期記憶 + 對話紀錄 + 提醒(M4 / M4.5)
│   │   └── scheduler.rs     # 提醒到期檢查、主動行為排程(M4.5)
│   ├── capabilities/        # 前端權限(Tauri v2 capability)
│   └── tauri.conf.json
├── src/                     # 前端(Vue 3 + TS)
│   ├── live2d/stage.ts      # 渲染、互動、閒置動作、情緒映射、口型同步
│   ├── llm/api.ts           # Rust LLM 命令/事件封裝(含取消)
│   ├── speech/              # tts.ts(Piper/系統語音)、recorder.ts(錄音→16kHz WAV)、native.ts(Rust 命令)
│   ├── passthrough.ts       # 智慧點擊穿透(游標輪詢 + 命中測試)
│   ├── chat/                # 對話輸入框、台詞泡泡
│   └── settings/            # 設定面板
├── public/
│   ├── vendor/              # 放 live2dcubismcore.min.js(見內部 README)
│   └── models/              # 放 Live2D 模型 + active.json(見內部 README)
└── docs/
```

## 授權注意事項

- **Cubism Core** 為 Live2D 專有軟體,須自行下載並同意其授權,不可隨本專案散布。
- **Live2D 模型**:本專案不附帶任何模型。請使用自有或已授權模型;官方免費素材受 Live2D Free Material License 約束。
- `pixi-live2d-display-lipsyncpatch` 為 MIT 授權,支援 lipsync(`model.speak()`),M2 對嘴功能將直接沿用。

## 開發備忘(後續里程碑)

- **語音架構(M2)**:sidecar 放 `%APPDATA%\com.desktoppet.ai\speech\`(`piper\piper.exe`+任一 `.onnx`;`whisper\whisper-cli.exe`+任一 `ggml-*.bin`),Rust `speech/mod.rs` 啟動子行程(隱藏主控台)。TTS 引擎為「自動」時 Piper 優先(WAV → blob URL → `model.speak()` 真實對嘴),否則 speechSynthesis + 正弦口型(`stage.ts` hookMouth)。STT:前端錄音 → WebAudio 重採樣 16kHz mono WAV → base64 → whisper-cli(`--prompt` 引導繁體輸出;base 模型仍常輸出簡體,辨識結果直接進 LLM 所以無妨,在意準確度可 `setup-speech.ps1 -WhisperModel small`)。
- **Agent 架構(M3)**:`agent/mod.rs` 持有工具規格(OpenAI tools 格式)與執行器;`provider.rs` 的 `run_agent_loop` 串流解析 `delta.tool_calls`(按 index 累積片段,容忍 arguments 為物件的非標準實作)→ 執行工具 → 以 `role:"tool"` 回填 → 再串流,上限 4 輪。權限:`PermissionState` 持 oneshot 通道,emit `agent-permission` → 前端卡片 → `agent_permission_response` 回填,timeout 60 秒。進入工具流程後不再降級(工具可能已有副作用)。新工具加在 `tool_specs()` + `execute()` 兩處即可。
- **記憶架構(M4)**:`memory/mod.rs` 兩張表 — memories(蒸餾事實,模型經 save_memory 工具寫入,最近 30 條 / 1500 字注入 system prompt,舊的用 search_memory LIKE 搜)、messages(對話紀錄,`load_recent_history` 在啟動時還原)。語意向量檢索與主動行為排程留 M4.5。
- **下一步(M3 後半 / M4.5)**:寫入類工具(寫檔、執行指令)+ 沙箱與更細的權限 UI;向量檢索;主動行為排程(到點提醒、閒置關心)。
- **情緒映射**:在 `public/models/active.json` 的 `emotions` 設定「標籤 → expression 名稱或 motion:群組名」;未設定時會嘗試同名表情,再退回隨機表情。
- **智慧穿透實作**:穿透狀態下視窗收不到滑鼠事件,所以 `src/passthrough.ts` 以 `cursorPosition()` 輪詢(120ms)+ `hitsModel()` 命中測試切換 `setIgnoreCursorEvents`;整窗手動穿透開啟時輪詢暫停(`click-through-manual` 事件)。
- **取消生成**:Rust `CancelState` 持有已取消的 requestId,串流迴圈每個 chunk 檢查;取消時以當下累積內容發 `chat-done` 收尾。
- **驗收基準**:M0(顯示/拖曳/托盤)✅;M1(文字對話、本地/雲端切換、斷線降級)✅ — 降級會在泡泡顯示「(雲端連不上,改用 Ollama)」;M2(回覆朗讀+對嘴、語音輸入)✅ — Piper/Whisper 未安裝時朗讀退回系統語音、語音輸入給安裝指引。
