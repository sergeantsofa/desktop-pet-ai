<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from "vue";
import {
  initStage,
  loadActiveModel,
  handleTap,
  hitsModel,
  markInteraction,
  setEmotion,
} from "./live2d/stage";
import {
  chatStream,
  cancelChat,
  healthCheck,
  loadRecentHistory,
  respondPermission,
  isTauri,
  type ChatMessage,
  type PermissionRequest,
} from "./llm/api";
import { speak, stopSpeaking } from "./speech/tts";
import { startRecording, stopRecording, cancelRecording } from "./speech/recorder";
import { transcribe } from "./speech/native";
import {
  startSmartPassthrough,
  stopSmartPassthrough,
  pauseSmartPassthrough,
} from "./passthrough";
import { loadBehavior } from "./behavior";
import ChatInput from "./chat/ChatInput.vue";
import SpeechBubble from "./chat/SpeechBubble.vue";
import PermissionPrompt from "./chat/PermissionPrompt.vue";
import SettingsPanel from "./settings/SettingsPanel.vue";

const canvasRef = ref<HTMLCanvasElement | null>(null);
const chatVisible = ref(false);
const settingsVisible = ref(false);
const bubbleText = ref("");
const loadError = ref("");
const muted = ref(false);
const thinking = ref(false);
let bubbleTimer: number | undefined;

/** 短期記憶:對話歷史(輪數截斷由 Rust 端處理) */
const history: ChatMessage[] = [];

function say(text: string, ms = 4500): void {
  if (bubbleTimer) window.clearTimeout(bubbleTimer);
  bubbleText.value = text;
  bubbleTimer = window.setTimeout(() => (bubbleText.value = ""), ms);
}

/** 串流期間直接更新泡泡,不自動消失 */
function sayStreaming(text: string): void {
  if (bubbleTimer) window.clearTimeout(bubbleTimer);
  bubbleText.value = text;
}

/* ---------- Tauri 事件(托盤/全域快捷鍵) ---------- */
async function setupTauriEvents(): Promise<void> {
  if (!isTauri) return;
  const { listen } = await import("@tauri-apps/api/event");
  await listen("toggle-chat", () => {
    chatVisible.value = !chatVisible.value;
  });
  await listen("open-settings", () => {
    settingsVisible.value = true;
  });
  await listen<boolean>("set-mute", (e) => {
    muted.value = e.payload;
    if (muted.value) {
      stopSpeaking();
    } else {
      say("我回來囉!");
    }
  });
  await listen("toggle-voice", () => void toggleVoice());
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
  // 智慧穿透:游標不在角色/UI 上時讓滑鼠穿透到下層(托盤開關)
  await listen<boolean>("smart-passthrough", (e) => {
    if (e.payload) {
      // 泡泡是純顯示、不可點,不列入 uiActive,以免說話期間擋住下層
      startSmartPassthrough({
        hits: hitsModel,
        uiActive: () =>
          chatVisible.value || settingsVisible.value || !!permission.value || !!loadError.value,
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
  /** 落地對話紀錄(主動關心/提醒傳 false) */
  persist: boolean;
  /** 出錯時是否在泡泡顯示(主動行為失敗應安靜,傳 false) */
  showError?: boolean;
}

/** 串流一輪對話並負責顯示/情緒/朗讀;回傳清乾淨的回覆(失敗回空字串)。 */
function streamReply(messages: ChatMessage[], opts: StreamOpts): Promise<string> {
  markActivity();
  thinking.value = true;
  sayStreaming("(思考中…)");

  let pending = "";
  let emotionDone = false;
  let prefix = "";

  return new Promise<string>((resolve) => {
    void chatStream(
      "chat",
      messages,
      {
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
          const clean = full.replace(EMOTION_RE, "").trim();
          say(prefix + (clean || "(…我詞窮了)"), Math.min(15000, 3000 + clean.length * 80));
          if (!muted.value && clean) void speak(clean);
          resolve(clean);
        },
        onError(message) {
          thinking.value = false;
          currentRequestId = null;
          if (opts.showError) {
            setEmotion("sad");
            say(`嗚…我的腦袋連不上:${message}`, 8000);
          } else if (bubbleText.value === "(思考中…)") {
            bubbleText.value = "";
          }
          resolve("");
        },
        onFallback(_from, to) {
          prefix = `(雲端連不上,改用${to})\n`;
        },
        onTool(label) {
          sayStreaming(`${prefix + pending}\n🔧(${label}中…)`.trim());
        },
      },
      opts.persist
    ).then((id) => (currentRequestId = id));
  });
}

async function onChatSubmit(text: string): Promise<void> {
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
  const clean = await streamReply([...history], { persist: true, showError: true });
  if (clean) history.push({ role: "assistant", content: clean });
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
    !settingsVisible.value &&
    !permission.value &&
    !recording.value &&
    !muted.value &&
    !loadError.value
  );
}

/** 送一則一次性指令給大腦(不落對話紀錄),讓她自然地說一句話 */
async function proactiveSay(instruction: string): Promise<void> {
  if (!canSpeakProactively()) return;
  markActivity();
  await streamReply([{ role: "user", content: instruction }], { persist: false });
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
  const clean = await streamReply(
    [
      {
        role: "user",
        content:
          `(系統:現在時間到了,請你提醒使用者這件事:「${content}」。` +
          `用你活潑可愛的口吻、簡短地提醒他,直接說話。)`,
      },
    ],
    { persist: false }
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

/* ---------- 語音輸入(M2:Ctrl+Shift+S) ---------- */
const recording = ref(false);

async function toggleVoice(): Promise<void> {
  if (!isTauri) {
    say("語音輸入要在桌面 App 裡才能用喔。");
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
onMounted(async () => {
  await setupTauriEvents();
  // M4:載入上次的對話,重啟後接得上話
  history.push(...(await loadRecentHistory()));
  const canvas = canvasRef.value!;
  initStage(canvas);

  let greeted = false;
  try {
    await loadActiveModel({ onSay: (line) => !muted.value && say(line) });
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
  startProactiveWatcher();
});

onBeforeUnmount(() => {
  if (proactiveTimer) window.clearInterval(proactiveTimer);
});
</script>

<template>
  <div
    class="stage"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
  >
    <canvas ref="canvasRef" class="live2d-canvas"></canvas>

    <!-- 模型尚未就緒時的引導占位角色 -->
    <div v-if="loadError" class="placeholder">
      <div class="placeholder-face">(。・ω・。)</div>
      <p class="placeholder-text">{{ loadError }}</p>
      <p class="placeholder-hint">放好檔案後重新啟動即可。詳見專案 README。(沒有模型也能聊天喔)</p>
    </div>

    <SpeechBubble v-if="bubbleText" :text="bubbleText" />
    <PermissionPrompt v-if="permission" :request="permission" @respond="onPermissionRespond" />
    <ChatInput v-if="chatVisible" @submit="onChatSubmit" @close="chatVisible = false" />
    <SettingsPanel v-if="settingsVisible" @close="settingsVisible = false" />
  </div>
</template>

<style scoped>
.stage {
  position: fixed;
  inset: 0;
  background: transparent;
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
</style>
