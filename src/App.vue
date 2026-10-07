<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from "vue";
import {
  initStage,
  loadActiveModel,
  handleTap,
  hitsModel,
  markInteraction,
  setEmotion,
  setSway,
  setUserScale,
  getHeadAnchor,
  switchModel,
} from "./live2d/stage";
import {
  chatStream,
  visionChat,
  visionChatFile,
  cancelChat,
  healthCheck,
  loadRecentHistory,
  respondPermission,
  onDiscordActivity,
  onRemoteActivity,
  hasApiKey,
  spotifyPlaybackState,
  gamekbAdd,
  isTauri,
  type ChatMessage,
  type PermissionRequest,
  type StreamHandlers,
} from "./llm/api";
import { speak, stopSpeaking } from "./speech/tts";
import { startRecording, stopRecording, cancelRecording } from "./speech/recorder";
import { startConverse, stopConverse, isConversing } from "./speech/converse";
import { transcribe } from "./speech/native";
import {
  startSmartPassthrough,
  stopSmartPassthrough,
  pauseSmartPassthrough,
} from "./passthrough";
import { loadBehavior, chimePrompt } from "./behavior";
import { checkForUpdate, updateTick, type UpdateInfo } from "./updater";
import { ensureCubismCore } from "./resources";
import { loadCharScale, loadBubbleStyle } from "./appearance";
import { pickTask } from "./llm/route";
import ChatInput from "./chat/ChatInput.vue";
import SpeechBubble from "./chat/SpeechBubble.vue";
import TalkSparkles from "./chat/TalkSparkles.vue";
import DevProgress from "./chat/DevProgress.vue";
import PermissionPrompt from "./chat/PermissionPrompt.vue";

const canvasRef = ref<HTMLCanvasElement | null>(null);
const chatVisible = ref(false);
const bubbleText = ref("");
/** 泡泡距視窗底部的距離(px),跟著角色頭頂走;undefined = 退回預設 top */
const bubbleBottom = ref<number | undefined>(undefined);
/** 泡泡淡出中(時間到 → 慢慢往上飄 + 淡出) */
const bubbleFading = ref(false);
let bubbleFadeTimer: number | undefined;
/** 對話泡泡樣式(設定面板可選,存 localStorage) */
const bubbleStyle = ref(loadBubbleStyle());
const loadError = ref("");
const muted = ref(false);
const thinking = ref(false);
let bubbleTimer: number | undefined;

/* ---------- 自我升級進度條(頭頂)---------- */
const devVisible = ref(false);
const devProgress = ref(0);
const devLabel = ref("");
const devState = ref<"working" | "done" | "failed">("working");
let devCreepTimer: number | undefined;
let devHideTimer: number | undefined;

/** 哪些工具算「自我升級」(會驅動頭頂進度條) */
const DEV_TOOLS = new Set([
  "dev_list_dir",
  "dev_read_file",
  "dev_write_file",
  "dev_run_check",
  "dev_revert",
]);

/** 各階段的目標百分比(進度只增不減) */
function devStageTarget(name: string): number {
  switch (name) {
    case "dev_list_dir":
    case "dev_read_file":
      return 40;
    case "dev_write_file":
    case "dev_revert":
      return 75;
    case "dev_run_check":
      return 92;
    default:
      return devProgress.value;
  }
}

/** 收到一個自我升級工具事件:顯示/推進進度條 */
function onDevTool(name: string, label: string): void {
  if (devHideTimer) {
    window.clearTimeout(devHideTimer);
    devHideTimer = undefined;
  }
  devVisible.value = true;
  devState.value = "working";
  devLabel.value = label;
  const target = devStageTarget(name);
  if (target > devProgress.value) devProgress.value = target;
  // 緩慢爬升(最多到 95),讓使用者感覺真的在動
  if (!devCreepTimer) {
    devCreepTimer = window.setInterval(() => {
      if (devState.value === "working" && devProgress.value < 95) {
        devProgress.value = Math.min(95, devProgress.value + 0.5);
      }
    }, 250);
  }
}

/** 這一輪結束:補到 100%(或標記失敗)後淡出 */
function finishDev(ok: boolean): void {
  if (!devVisible.value) return;
  if (devCreepTimer) {
    window.clearInterval(devCreepTimer);
    devCreepTimer = undefined;
  }
  if (ok) {
    devProgress.value = 100;
    devState.value = "done";
  } else {
    devState.value = "failed";
  }
  devHideTimer = window.setTimeout(
    () => {
      devVisible.value = false;
      devProgress.value = 0;
      devLabel.value = "";
      devState.value = "working";
    },
    ok ? 1800 : 2800
  );
}

/** 短期記憶:對話歷史(輪數截斷由 Rust 端處理) */
const history: ChatMessage[] = [];

/** 收到設定視窗的換角色指令 → 載入該角色模型 */
async function switchCharacterById(id: string): Promise<void> {
  try {
    const { loadCharacters } = await import("./live2d/characters");
    const cfg = (await loadCharacters()).find((c) => c.id === id);
    if (cfg) {
      await switchModel(cfg);
      setUserScale(loadCharScale()); // 換角色後沿用目前的角色大小
    }
  } catch {
    /* 換角色失敗就維持原樣 */
  }
}

/** 讓泡泡對齊角色頭頂上方;沒有模型(引導畫面)就退回預設 top 位置 */
function refreshBubbleAnchor(): void {
  const a = getHeadAnchor();
  if (!a) {
    bubbleBottom.value = undefined;
    return;
  }
  const GAP = 10; // 泡泡底端離頭頂的間距
  const raw = window.innerHeight - a.y + GAP;
  // 不要黏到最底,也不要整個衝出視窗頂端(頭在最上面時泡泡仍留在上方一點點)
  bubbleBottom.value = Math.min(window.innerHeight - 50, Math.max(20, raw));
}

function say(text: string, ms = 4500): void {
  if (bubbleTimer) window.clearTimeout(bubbleTimer);
  if (bubbleFadeTimer) window.clearTimeout(bubbleFadeTimer);
  bubbleFading.value = false;
  refreshBubbleAnchor();
  bubbleText.value = text;
  bubbleTimer = window.setTimeout(() => {
    bubbleFading.value = true; // 時間到 → 慢慢往上飄 + 淡出
    bubbleFadeTimer = window.setTimeout(() => {
      bubbleText.value = "";
      bubbleFading.value = false;
    }, 800);
  }, ms);
}

/** 串流期間直接更新泡泡,不自動消失 */
function sayStreaming(text: string): void {
  if (bubbleTimer) window.clearTimeout(bubbleTimer);
  if (bubbleFadeTimer) window.clearTimeout(bubbleFadeTimer);
  bubbleFading.value = false;
  refreshBubbleAnchor();
  bubbleText.value = text;
}

/** 視窗縮放後角色會重新佈局,泡泡顯示中就跟著重新對齊頭頂(等 fitModel 跑完) */
function onWindowResize(): void {
  if (!bubbleText.value) return;
  requestAnimationFrame(refreshBubbleAnchor);
}

/**
 * 顯示泡泡並朗讀,泡泡會「撐到語音講完」才收(講完再多留 ~1.2s)。
 * 靜音或沒有可朗讀內容時,退回用 fallbackMs 的估時。
 * speakSeq 保證舊語音的結束回呼不會誤收新訊息的泡泡。
 */
let speakSeq = 0;
function sayWhileSpeaking(displayText: string, spokenText: string, fallbackMs: number): void {
  if (muted.value || !spokenText) {
    say(displayText, fallbackMs);
    return;
  }
  say(displayText, 30000); // 長 fallback;TTS 結束會由 onEnd 縮短
  const id = ++speakSeq;
  void speak(spokenText, undefined, (spoke) => {
    if (id !== speakSeq) return; // 已被新訊息取代,不要亂收
    say(displayText, spoke ? 1200 : fallbackMs);
  });
}

/**
 * 角色自發台詞(點擊/閒置)→ 顯示泡泡 + 朗讀。
 * 朗讀時濾掉括號內的動作描述(如「(東張西望)」),純動作描述則只顯示不唸。
 */
function onPetLine(line: string): void {
  if (muted.value) return; // 靜音:泡泡碎念與朗讀都關(與托盤靜音一致)
  const spoken = line.replace(/[（(][^）)]*[）)]/g, "").trim();
  // 純動作描述(如「(東張西望)」)只顯示不唸 → 用估時;有台詞則撐到唸完
  sayWhileSpeaking(line, spoken, 4500);
}

/* ---------- Tauri 事件(托盤/全域快捷鍵) ---------- */
async function setupTauriEvents(): Promise<void> {
  if (!isTauri) return;
  const { listen } = await import("@tauri-apps/api/event");
  await listen("toggle-chat", () => {
    chatVisible.value = !chatVisible.value;
  });
  // 設定視窗(獨立視窗)改了外觀 → 即時套用到桌寵本體
  await listen<number>("pet:set-scale", (e) => setUserScale(e.payload));
  await listen<string>("pet:set-bubble", (e) => {
    bubbleStyle.value = e.payload;
  });
  await listen<string>("pet:switch-character", (e) => void switchCharacterById(e.payload));
  await listen<boolean>("set-mute", (e) => {
    muted.value = e.payload;
    if (muted.value) {
      stopSpeaking();
    } else {
      say("我回來囉!");
    }
  });
  await listen("toggle-voice", () => void toggleVoice());
  // 連續對話模式(Ctrl+Shift+D):開著麥克風持續聽,靜音斷句、開口打斷
  await listen("toggle-converse", () => void toggleConverse());
  // M3:工具權限請求(Rust 等待 60 秒,逾時自動拒絕並發 close)
  await listen<PermissionRequest>("agent-permission", (e) => {
    permission.value = e.payload;
  });
  await listen<{ callId: string }>("agent-permission-close", (e) => {
    if (permission.value?.callId === e.payload.callId) permission.value = null;
  });
  // M4.5:提醒到期 → 主動跳出來提醒
  await listen<{ content: string }>("reminder-due", (e) => {
    void onReminderDue(e.payload.content);
  });
  // M5:看截圖(Ctrl+Shift+V / 托盤)
  await listen("see-screen", () => void seeScreen());
  // M5.5:截圖資料夾出現新圖 → 自動讀圖評論
  await listen<{ path: string }>("screenshot-added", (e) => {
    void onNewScreenshot(e.payload.path);
  });
  // 智慧穿透:游標不在角色/UI 上時讓滑鼠穿透到下層(托盤開關)
  await listen<boolean>("smart-passthrough", (e) => {
    if (e.payload) {
      // 泡泡是純顯示、不可點,不列入 uiActive,以免說話期間擋住下層
      startSmartPassthrough({
        hits: hitsModel,
        uiActive: () =>
          chatVisible.value || !!permission.value || !!loadError.value,
      });
    } else {
      void stopSmartPassthrough();
    }
  });
  // 整窗手動穿透優先,智慧輪詢暫停
  await listen<boolean>("click-through-manual", (e) => {
    pauseSmartPassthrough(e.payload);
  });
}

/* ---------- 拖曳 vs 點擊 ---------- */
const DRAG_THRESHOLD = 6;
let downPos: { x: number; y: number } | null = null;
let dragging = false;

function onPointerDown(e: PointerEvent): void {
  if (e.button !== 0) return;
  downPos = { x: e.screenX, y: e.screenY };
  dragging = false;
  markInteraction();
  markActivity();
}

async function onPointerMove(e: PointerEvent): Promise<void> {
  if (!downPos || dragging) return;
  if (Math.hypot(e.screenX - downPos.x, e.screenY - downPos.y) > DRAG_THRESHOLD) {
    dragging = true;
    downPos = null;
    if (isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().startDragging();
    }
  }
}

function onPointerUp(e: PointerEvent): void {
  if (downPos && !dragging) {
    handleTap(e.clientX, e.clientY);
  }
  downPos = null;
  dragging = false;
}

/* ---------- 對話(M1:接上大腦) ---------- */

/** 解析串流開頭的情緒標籤 */
const EMOTION_RE = /^\s*\[(\w+)\]\s*/;

let currentRequestId: string | null = null;

interface StreamOpts {
  /** 出錯時是否在泡泡顯示(主動行為失敗應安靜,傳 false) */
  showError?: boolean;
  /** 思考中的占位字(看截圖時顯示「看看…」) */
  thinkingText?: string;
}

/**
 * 串流一輪回應並負責顯示/情緒/朗讀;回傳清乾淨的回覆(失敗回空字串)。
 * `start` 啟動底層串流(對話 chatStream / 看圖 visionChat),回傳 requestId。
 */
function streamReply(
  start: (handlers: StreamHandlers) => Promise<string>,
  opts: StreamOpts = {}
): Promise<string> {
  markActivity();
  thinking.value = true;
  const placeholder = opts.thinkingText ?? "(思考中…)";
  sayStreaming(placeholder);

  let pending = "";
  let emotionDone = false;
  let prefix = "";

  return new Promise<string>((resolve) => {
    void start({
      onDelta(delta) {
        pending += delta;
        if (!emotionDone) {
          const m = pending.match(EMOTION_RE);
          if (m) {
            emotionDone = true;
            setEmotion(m[1]);
            pending = pending.replace(EMOTION_RE, "");
          } else if (pending.length > 16) {
            emotionDone = true; // 模型沒給標籤,放棄等待
          } else {
            return; // 標籤可能還沒收完,先不顯示
          }
        }
        sayStreaming(prefix + pending);
      },
      onDone(full) {
        thinking.value = false;
        currentRequestId = null;
        if (devVisible.value) finishDev(true);
        const clean = full.replace(EMOTION_RE, "").trim();
        const display = prefix + (clean || "(…我詞窮了)");
        // 泡泡撐到語音講完才收;靜音/無內容時用原本的估時
        sayWhileSpeaking(display, clean, Math.min(15000, 3000 + clean.length * 80));
        resolve(clean);
      },
      onError(message) {
        thinking.value = false;
        currentRequestId = null;
        if (devVisible.value) finishDev(false);
        if (opts.showError) {
          setEmotion("sad");
          say(`嗚…我的腦袋連不上:${message}`, 8000);
        } else if (bubbleText.value === placeholder) {
          bubbleText.value = "";
        }
        resolve("");
      },
      onFallback(_from, to) {
        prefix = `(雲端連不上,改用${to})\n`;
      },
      onTool(label, name) {
        // 自我升級工具 → 頭頂進度條;其他工具 → 維持泡泡提示
        if (DEV_TOOLS.has(name)) {
          onDevTool(name, label);
        } else {
          sayStreaming(`${prefix + pending}\n🔧(${label}中…)`.trim());
        }
      },
    }).then((id) => (currentRequestId = id));
  });
}

async function onChatSubmit(text: string): Promise<void> {
  // 「教:○○○」→ 教她一條遊戲知識(存進知識庫,不進對話)。遊戲知識庫模式靠這個慢慢累積。
  const teach = text.match(/^\s*教\s*[:：]\s*([\s\S]+)/);
  if (teach && teach[1].trim()) {
    chatVisible.value = false;
    const fact = teach[1].trim();
    try {
      await gamekbAdd(fact);
      setEmotion("happy");
      say(`📚 學起來了!「${fact.length > 18 ? fact.slice(0, 18) + "…" : fact}」`, 4500);
    } catch (err) {
      say(`咦,我記不起來耶…${err}`, 5000);
    }
    return;
  }
  // 思考中再送訊息 → 打斷舊回應,直接回答新的
  if (currentRequestId) {
    cancelChat(currentRequestId);
    currentRequestId = null;
  }
  // 錄音中改用打字 → 放棄錄音
  if (recording.value) {
    cancelRecording();
    recording.value = false;
  }
  stopSpeaking();
  chatVisible.value = false;

  history.push({ role: "user", content: text });
  const snapshot = [...history];
  // 自動分流:依這句內容選 chat / coder / reasoner(主動找話題、提醒仍固定走 chat)
  const task = pickTask(text);
  const clean = await streamReply((h) => chatStream(task, snapshot, h, true), {
    showError: true,
  });
  if (clean) history.push({ role: "assistant", content: clean });
}

/* ---------- 看截圖(M5:Ctrl+Shift+V / 托盤) ---------- */
async function seeScreen(): Promise<void> {
  if (!isTauri) {
    say("看螢幕要在桌面 App 裡才能用喔。");
    return;
  }
  if (currentRequestId || thinking.value) return; // 忙碌中不重入
  markActivity();
  setEmotion("surprised");
  // 視覺模型不落對話紀錄(圖無法重現);prompt 留空 → Rust 用預設
  await streamReply((h) => visionChat("", h), {
    showError: true,
    thinkingText: "(讓我看看…📷)",
  });
}

/** 截圖資料夾出現新圖(自動觸發);她忙碌時就略過這張,不打斷。 */
async function onNewScreenshot(path: string): Promise<void> {
  if (!isTauri) return;
  if (currentRequestId || thinking.value || chatVisible.value || recording.value) return;
  markActivity();
  setEmotion("surprised");
  await streamReply((h) => visionChatFile(path, "", h), {
    thinkingText: "(咦,你截圖了?讓我看看…📷)",
  });
}

/* ---------- 主動行為(M4.5) ---------- */
let lastActivity = Date.now();
let proactiveTimer: number | undefined;

/** 使用者有任何互動就重置主動計時 */
function markActivity(): void {
  lastActivity = Date.now();
}

/** 是否可以插話(沒在忙、沒開面板、沒靜音) */
function canSpeakProactively(): boolean {
  return (
    !thinking.value &&
    !currentRequestId &&
    !chatVisible.value &&
    !permission.value &&
    !recording.value &&
    !conversing.value &&
    !muted.value &&
    !loadError.value
  );
}

/** 送一則一次性指令給大腦(不落對話紀錄),讓她自然地說一句話 */
async function proactiveSay(instruction: string): Promise<void> {
  if (!canSpeakProactively()) return;
  markActivity();
  await streamReply((h) => chatStream("chat", [{ role: "user", content: instruction }], h, false));
}

function startProactiveWatcher(): void {
  if (!isTauri) return;
  proactiveTimer = window.setInterval(() => {
    const behavior = loadBehavior();
    if (!behavior.proactiveChat) return;
    if (Date.now() - lastActivity < behavior.idleMinutes * 60_000) return;
    if (!canSpeakProactively()) return;
    markActivity(); // 先重置,避免連續觸發
    void proactiveSay(
      "(系統:使用者已經有一段時間沒理你了。請你主動、自然、簡短地說一句話——" +
        "關心他、分享心情、或聊聊你記得關於他的事。直接說話,不要提到這是系統訊息。)"
    );
  }, 60_000);
}

/* ---------- 整點報時 ---------- */
let chimeTimer: number | undefined;

function startChimeWatcher(): void {
  if (!isTauri) return;
  // 每分鐘檢查一次是否到了新的整點
  chimeTimer = window.setInterval(() => {
    const behavior = loadBehavior();
    if (!behavior.hourlyChime) return;
    if (!canSpeakProactively()) return;

    const now = new Date();
    // 只在第一分鐘觸發(分==0);用秒判斷避免連續觸發
    if (now.getMinutes() !== 0) return;
    if (now.getSeconds() > 30) return; // 避免整點後的延遲觸發

    const hour = now.getHours();
    const prompt = chimePrompt(hour, behavior.chimeStyle);
    // 用 proactiveSay 避開忙碌狀態
    void proactiveSay(prompt);
  }, 60_000);
}

/* ---------- 跟音樂搖擺(Spotify) ---------- */
let swayTimer: number | undefined;

function startSwayWatcher(): void {
  if (!isTauri) return;
  const poll = async () => {
    const st = await spotifyPlaybackState();
    setSway(st.playing, st.bpm ?? 0);
  };
  void poll();
  // 每 5 秒問一次目前播放狀態(換歌時 BPM 也會跟著更新)
  swayTimer = window.setInterval(() => void poll(), 5_000);
}

/* ---------- 提醒到期(M4.5) ---------- */
async function onReminderDue(content: string): Promise<void> {
  // 提醒一定要送達:先把視窗叫到前景
  if (isTauri) {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    await win.show();
    await win.setFocus();
  }
  setEmotion("surprised");
  // 讓她用自己的口吻講出來;大腦不通就退回固定句
  const spoken = await proactiveSayReminder(content);
  if (!spoken) {
    const line = `叮咚!時間到囉~ 你要我提醒你:「${content}」!`;
    say(line, 12000);
    if (!muted.value) void speak(line);
  }
}

/** 用大腦把提醒講得有生命力;回傳是否成功 */
async function proactiveSayReminder(content: string): Promise<boolean> {
  if (thinking.value || currentRequestId || muted.value) return false;
  markActivity();
  const instruction =
    `(系統:現在時間到了,請你提醒使用者這件事:「${content}」。` +
    `用你活潑可愛的口吻、簡短地提醒他,直接說話。)`;
  const clean = await streamReply((h) =>
    chatStream("chat", [{ role: "user", content: instruction }], h, false)
  );
  return clean.length > 0;
}

/* ---------- Agent 權限(M3) ---------- */
const permission = ref<PermissionRequest | null>(null);

function onPermissionRespond(allow: boolean): void {
  if (!permission.value) return;
  respondPermission(permission.value.callId, allow);
  permission.value = null;
}

/* ---------- 連續對話模式(Ctrl+Shift+D) ---------- */
const conversing = ref(false);

async function toggleConverse(): Promise<void> {
  if (!isTauri) {
    say("連續對話要在桌面 App 裡才能用喔。");
    return;
  }
  if (conversing.value) {
    stopConverse();
    conversing.value = false;
    say("好,連續對話結束囉~", 4000);
    return;
  }
  // 進連續對話前,先停掉一次性錄音(避免搶麥克風)
  if (recording.value) {
    cancelRecording();
    recording.value = false;
  }
  markInteraction();
  markActivity();
  try {
    const silenceSec = loadBehavior().converseSilenceSec ?? 2;
    await startConverse({
      silenceMs: Math.max(500, Math.round(silenceSec * 1000)),
      onSpeechStart: onConverseSpeechStart,
      onUtterance: (wav) => void onConverseUtterance(wav),
    });
    conversing.value = true;
    setEmotion("happy");
    say("🎙️ 連續對話開始!直接跟我說話,停一下我就回你~(再按 Ctrl+Shift+D 結束)", 6000);
  } catch {
    say("麥克風打不開耶…檢查一下裝置或權限?", 6000);
  }
}

/** 連續對話中:使用者一開口 → 桌寵立刻閉嘴、停掉正在生成的回答,等你說完 */
function onConverseSpeechStart(): void {
  stopSpeaking();
  if (currentRequestId) {
    cancelChat(currentRequestId);
    currentRequestId = null;
  }
  thinking.value = false;
  markActivity();
}

/** 連續對話中:一段話講完(靜音斷句)→ 辨識 → 走一般對話流程回答 */
async function onConverseUtterance(wav: Uint8Array): Promise<void> {
  if (!conversing.value) return;
  markActivity();
  let text = "";
  try {
    text = (await transcribe(wav)).trim();
  } catch {
    return; // 辨識失敗就略過這句,繼續聽
  }
  if (!text) return;
  await onChatSubmit(text);
}

/* ---------- 語音輸入(M2:Ctrl+Shift+S) ---------- */
const recording = ref(false);

async function toggleVoice(): Promise<void> {
  if (!isTauri) {
    say("語音輸入要在桌面 App 裡才能用喔。");
    return;
  }
  if (conversing.value) {
    say("連續對話進行中~(按 Ctrl+Shift+D 結束後再用單次語音)", 4000);
    return;
  }
  markInteraction();
  markActivity();
  if (!recording.value) {
    try {
      await startRecording((wav) => {
        // 錄滿上限自動結束
        recording.value = false;
        void submitVoice(wav);
      });
      recording.value = true;
      sayStreaming("🎤(聆聽中…再按 Ctrl+Shift+S 結束)");
    } catch {
      say("麥克風打不開耶…檢查一下裝置或權限?", 6000);
    }
    return;
  }
  recording.value = false;
  try {
    const wav = await stopRecording();
    await submitVoice(wav);
  } catch (err) {
    say(`錄音處理失敗:${err instanceof Error ? err.message : err}`, 6000);
  }
}

async function submitVoice(wav: Uint8Array): Promise<void> {
  sayStreaming("(辨識中…)");
  try {
    const text = (await transcribe(wav)).trim();
    if (!text) {
      say("我沒聽清楚耶,再說一次?");
      return;
    }
    await onChatSubmit(text);
  } catch (err) {
    say(`語音辨識失敗:${err}`, 8000);
  }
}

/* ---------- 啟動 ---------- */
/* ---------- 滑鼠座標顯示(左上角) ---------- */
/** 目前滑鼠的全域螢幕座標文字;空字串 = 還沒讀到 */
const cursorXY = ref("");
let cursorTimer: number | undefined;

/** 輪詢全域游標位置(物理像素),顯示在視窗左上角。穿透狀態也照常運作(只是查詢 OS,不靠滑鼠事件)。 */
async function startCursorReadout(): Promise<void> {
  if (!isTauri || cursorTimer) return;
  const { cursorPosition } = await import("@tauri-apps/api/window");
  cursorTimer = window.setInterval(async () => {
    try {
      const p = await cursorPosition();
      cursorXY.value = `X ${Math.round(p.x)}  Y ${Math.round(p.y)}`;
    } catch {
      /* 視窗關閉中等暫態錯誤,下一輪再試 */
    }
  }, 80);
}

onMounted(async () => {
  await setupTauriEvents();
  // M4:載入上次的對話,重啟後接得上話
  history.push(...(await loadRecentHistory()));
  const canvas = canvasRef.value!;
  initStage(canvas);

  // 確保 Cubism Core 已載入(背景外部 vendor);失敗/逾時不擋 UI,
  // loadActiveModel 會因缺 Core 而顯示引導畫面(含一鍵下載)。
  try {
    await ensureCubismCore();
  } catch {
    /* 載不到就讓 loadActiveModel 走引導流程 */
  }

  let greeted = false;
  try {
    await loadActiveModel({ onSay: onPetLine });
    setUserScale(loadCharScale()); // 套用上次存的角色大小
  } catch (err) {
    loadError.value = err instanceof Error ? err.message : String(err);
  }

  // 健康檢查:Ollama 沒開就引導
  if (isTauri) {
    try {
      const health = await healthCheck();
      if (!health.ollama) {
        greeted = true;
        say(
          "找不到 Ollama 耶…請先安裝並啟動(winget install Ollama.Ollama,再 ollama pull qwen2.5:7b),或在設定裡填 DeepSeek Key 用雲端腦袋。",
          12000
        );
      }
    } catch {
      /* 健康檢查失敗不擋啟動 */
    }
  }
  if (!greeted && !loadError.value) {
    say("嗨!我醒來了~ 按 Ctrl+Shift+A 跟我聊天吧!");
  }

  markActivity();
  window.addEventListener("resize", onWindowResize);
  startProactiveWatcher();
  startChimeWatcher(); // ← 啟動整點報時

  // Discord 動靜:她在頻道回了話 → 桌面冒個泡泡(不打斷本地互動、不朗讀)
  void onDiscordActivity((info) => {
    if (thinking.value || chatVisible.value || recording.value) return;
    const snip = info.reply.length > 24 ? info.reply.slice(0, 24) + "…" : info.reply;
    setEmotion("happy");
    say(`💬 有人在 Discord 找我~ 我回:「${snip}」`, 4500);
  });

  // 遠端網頁動靜:別台電腦/手機在區網跟她聊了 → 桌面冒個泡泡(不打斷本地互動、不朗讀)
  void onRemoteActivity((info) => {
    if (thinking.value || chatVisible.value || recording.value) return;
    const snip = info.reply.length > 24 ? info.reply.slice(0, 24) + "…" : info.reply;
    setEmotion("happy");
    say(`🌐 有人從別台電腦找我~ 我回:「${snip}」`, 4500);
  });

  // 跟音樂搖擺:連了 Spotify 才輪詢播放狀態,播放中就讓她隨節奏擺動
  if (isTauri && (await hasApiKey("spotify"))) {
    startSwayWatcher();
  }

  // 左上角即時顯示滑鼠座標
  void startCursorReadout();

  // 自動更新:啟動後靜默檢查(24h 節流)。
  // 開了「自動下載」就整個跑完(下載→安裝→重啟),否則只冒泡泡提示。
  void updateTick((text) => {
    if (text) say(text, 6000);
  }).then(async (result) => {
    if (result !== "notified") return;
    const info = await checkForUpdate();
    if (!info) return;
    update.value = info;
    say(`欸!我有新版本可以更新喔(v${info.version})~`, 8000);
  });
});

onBeforeUnmount(() => {
  if (isConversing()) stopConverse();
  window.removeEventListener("resize", onWindowResize);
  if (proactiveTimer) window.clearInterval(proactiveTimer);
  if (chimeTimer) window.clearInterval(chimeTimer); // ← 清除整點報時
  if (swayTimer) window.clearInterval(swayTimer);
  if (devCreepTimer) window.clearInterval(devCreepTimer);
  if (devHideTimer) window.clearTimeout(devHideTimer);
  if (cursorTimer) window.clearInterval(cursorTimer);
});

/* ---------- 自動更新 ---------- */
const update = ref<UpdateInfo | null>(null);
const updating = ref(false);

async function openDataFolder(): Promise<void> {
  if (!isTauri) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("open_data_folder");
  } catch {
    /* 開資料夾失敗忽略 */
  }
}

/* ---------- 一鍵設定(首次啟動下載範例角色 + Core) ---------- */
const bootstrapping = ref(false);
const bootstrapMsg = ref("");

async function oneClickSetup(): Promise<void> {
  if (!isTauri || bootstrapping.value) return;
  bootstrapping.value = true;
  bootstrapMsg.value = "準備中…";
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<{ done: number; total: number; label: string }>(
      "bootstrap-progress",
      (e) => {
        const { done, total, label } = e.payload;
        bootstrapMsg.value = `${label}(${done}/${total})`;
      }
    );
    await invoke("bootstrap_assets");
    un();
    bootstrapMsg.value = "完成!正在載入…";
    // 重載前端 → 重新載入(這次外部已有 Core + 模型)
    window.location.reload();
  } catch (err) {
    bootstrapping.value = false;
    bootstrapMsg.value = `下載失敗:${err}`;
  }
}

async function onUpdateNow(): Promise<void> {
  if (!update.value) return;
  updating.value = true;
  sayStreaming("(下載更新中…裝好我會自己重開)");
  try {
    await update.value.install();
  } catch (err) {
    updating.value = false;
    say(`更新失敗了…${err}`, 8000);
  }
}
</script>

<template>
  <div
    class="stage"
    :class="{ 'dev-busy': devVisible }"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
  >
    <canvas ref="canvasRef" class="live2d-canvas"></canvas>

    <!-- 左上角:即時滑鼠座標(純顯示,不擋互動) -->
    <div v-if="cursorXY" class="cursor-readout">{{ cursorXY }}</div>

    <!-- 模型尚未就緒時的引導占位角色 -->
    <div v-if="loadError" class="placeholder">
      <div class="placeholder-face">(。・ω・。)</div>
      <template v-if="isTauri">
        <p class="placeholder-text">還沒有角色~ 點下面讓我幫你下載一個範例角色就能開始!</p>
        <button class="setup-btn" :disabled="bootstrapping" @pointerdown.stop @click="oneClickSetup">
          {{ bootstrapping ? bootstrapMsg : "✨ 一鍵下載範例角色(約 5MB)" }}
        </button>
        <button class="folder-btn" @pointerdown.stop @click="openDataFolder">
          📁 我要放自己的模型(開啟資料夾)
        </button>
        <p class="placeholder-hint">想換自己的 Live2D 模型?詳見專案 README。</p>
      </template>
      <template v-else>
        <p class="placeholder-text">{{ loadError }}</p>
      </template>
    </div>

    <DevProgress
      v-if="devVisible"
      :label="devLabel"
      :progress="devProgress"
      :state="devState"
    />
    <TalkSparkles v-if="bubbleText" />
    <SpeechBubble
      v-if="bubbleText"
      :text="bubbleText"
      :bottom="bubbleBottom"
      :variant="bubbleStyle"
      :fading="bubbleFading"
    />
    <PermissionPrompt v-if="permission" :request="permission" @respond="onPermissionRespond" />

    <!-- 自動更新提示 -->
    <div v-if="update && !updating" class="update-bar" @pointerdown.stop>
      <span>新版本 v{{ update.version }} 可用</span>
      <div class="update-actions">
        <button class="update-now" @click="onUpdateNow">更新</button>
        <button class="update-later" @click="update = null">稍後</button>
      </div>
    </div>

    <ChatInput v-if="chatVisible" @submit="onChatSubmit" @close="chatVisible = false" />
  </div>
</template>

<style scoped>
.stage {
  position: fixed;
  inset: 0;
  background: transparent;
}
/* 左上角滑鼠座標:純顯示,不攔截滑鼠(pointer-events:none),穿透/拖曳都不受影響 */
.cursor-readout {
  position: fixed;
  top: 6px;
  left: 8px;
  z-index: 9999;
  padding: 2px 8px;
  border-radius: 8px;
  background: rgba(0, 0, 0, 0.55);
  color: #fff;
  font: 12px/1.4 "Consolas", "Cascadia Mono", monospace;
  letter-spacing: 0.5px;
  white-space: nowrap;
  pointer-events: none;
  user-select: none;
}
/* 自我升級進度條顯示時,把對話泡泡往下挪,避免疊在一起 */
.stage.dev-busy :deep(.bubble) {
  top: 58px;
}
.update-bar {
  position: absolute;
  top: 8px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 10px;
  background: rgba(91, 141, 239, 0.96);
  color: #fff;
  padding: 6px 10px 6px 14px;
  border-radius: 16px;
  font-size: 12px;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.3);
  white-space: nowrap;
}
.update-actions {
  display: flex;
  gap: 6px;
}
.update-bar button {
  border: none;
  border-radius: 12px;
  padding: 4px 12px;
  cursor: pointer;
  font-size: 12px;
}
.update-now {
  background: #fff;
  color: #4a7de0;
  font-weight: 600;
}
.update-later {
  background: rgba(255, 255, 255, 0.25);
  color: #fff;
}
.live2d-canvas {
  position: absolute;
  inset: 0;
}
.placeholder {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 24px;
  text-align: center;
}
.placeholder-face {
  font-size: 42px;
  background: #fff;
  border-radius: 50%;
  width: 140px;
  height: 140px;
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
}
.placeholder-text {
  max-width: 320px;
  background: rgba(255, 255, 255, 0.95);
  border-radius: 12px;
  padding: 10px 14px;
  font-size: 13px;
  line-height: 1.6;
  color: #333;
}
.placeholder-hint {
  font-size: 11px;
  color: #f5f5f5;
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.6);
}
.setup-btn {
  border: none;
  background: #5b8def;
  color: #fff;
  padding: 10px 20px;
  border-radius: 18px;
  cursor: pointer;
  font-size: 14px;
  font-weight: 600;
  box-shadow: 0 4px 14px rgba(91, 141, 239, 0.5);
}
.setup-btn:hover {
  background: #4a7de0;
}
.setup-btn:disabled {
  background: #9bb5e8;
  cursor: default;
}
.folder-btn {
  border: 1px solid rgba(255, 255, 255, 0.7);
  background: transparent;
  color: #fff;
  padding: 6px 14px;
  border-radius: 14px;
  cursor: pointer;
  font-size: 12px;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.5);
}
.folder-btn:hover {
  background: rgba(255, 255, 255, 0.15);
}
</style>
