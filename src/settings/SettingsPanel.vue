<script setup lang="ts">
import { onMounted, ref } from "vue";
import {
  getSettings,
  saveSettings,
  setApiKey,
  hasApiKey,
  clearMemories,
  clearHistory,
  isTauri,
  type Settings,
} from "../llm/api";
import {
  loadTtsSettings,
  saveTtsSettings,
  listVoices,
  ttsSupported,
  speak,
  EDGE_VOICES,
  type TtsSettings,
} from "../speech/tts";
import { speechStatus, type SpeechStatus } from "../speech/native";
import { loadBehavior, saveBehavior, type BehaviorSettings } from "../behavior";
import {
  loadCharacters,
  getActiveCharacterId,
  setActiveCharacterId,
  type Character,
} from "../live2d/characters";
import { switchModel } from "../live2d/stage";

const emit = defineEmits<{ (e: "close"): void; (e: "saved"): void }>();

const settings = ref<Settings | null>(null);
const deepseekKey = ref("");
const deepseekKeySet = ref(false);
const status = ref("");
const tts = ref<TtsSettings>(loadTtsSettings());
const voices = ref<SpeechSynthesisVoice[]>([]);
const speech = ref<SpeechStatus | null>(null);
const behavior = ref<BehaviorSettings>(loadBehavior());
const characters = ref<Character[]>([]);
const selectedCharacter = ref("");
const switchingCharacter = ref(false);
const TASKS: Array<{ id: string; label: string }> = [
  { id: "chat", label: "閒聊" },
  { id: "coder", label: "寫程式" },
  { id: "reasoner", label: "推理" },
  { id: "vision", label: "看圖" },
];

/** 舊設定檔可能缺某些任務路由,補上預設以免 UI 綁定到 undefined */
const ROUTE_DEFAULTS: Record<string, { provider: string; model: string }> = {
  chat: { provider: "ollama", model: "qwen2.5:7b" },
  coder: { provider: "ollama", model: "qwen2.5-coder:7b" },
  reasoner: { provider: "ollama", model: "deepseek-r1:7b" },
  vision: { provider: "ollama", model: "qwen2.5vl:3b" },
};

function ensureRoutes(s: Settings): void {
  for (const t of TASKS) {
    if (!s.routing[t.id]) s.routing[t.id] = { ...ROUTE_DEFAULTS[t.id] };
  }
}

onMounted(async () => {
  characters.value = await loadCharacters();
  selectedCharacter.value = await getActiveCharacterId();
  if (ttsSupported()) {
    // 中文語音排前面,挑選方便
    const all = await listVoices();
    voices.value = [
      ...all.filter((v) => /^zh/i.test(v.lang)),
      ...all.filter((v) => !/^zh/i.test(v.lang)),
    ];
  }
  if (isTauri) speech.value = await speechStatus();
  if (!isTauri) {
    status.value = "非 Tauri 環境,設定僅供預覽";
    settings.value = {
      providers: [
        { id: "ollama", name: "Ollama(本地)", base_url: "http://localhost:11434/v1", uses_key: false },
        { id: "deepseek", name: "DeepSeek(雲端)", base_url: "https://api.deepseek.com", uses_key: true },
      ],
      routing: {
        chat: { provider: "ollama", model: "qwen2.5:7b" },
        coder: { provider: "ollama", model: "qwen2.5-coder:7b" },
        reasoner: { provider: "ollama", model: "deepseek-r1:7b" },
      },
      fallback: { provider: "ollama", model: "qwen2.5:7b" },
      fallback_to_local: true,
      persona: { name: "小桌寵", system_prompt: "" },
      context_turns: 10,
      agent_enabled: true,
      watch_screenshots: false,
      screenshot_dir: "",
      self_dev_enabled: false,
      self_dev_root: "",
    };
    ensureRoutes(settings.value);
    return;
  }
  const loaded = await getSettings();
  ensureRoutes(loaded);
  settings.value = loaded;
  deepseekKeySet.value = await hasApiKey("deepseek");
});

async function save(): Promise<void> {
  if (!settings.value) return;
  try {
    saveTtsSettings(tts.value);
    saveBehavior(behavior.value);
    if (isTauri) {
      await saveSettings(settings.value);
      if (deepseekKey.value.trim()) {
        await setApiKey("deepseek", deepseekKey.value);
        deepseekKey.value = "";
        deepseekKeySet.value = true;
      }
    }
    status.value = "已儲存!";
    emit("saved");
    window.setTimeout(() => (status.value = ""), 2000);
  } catch (err) {
    status.value = `儲存失敗:${err}`;
  }
}

async function onChangeCharacter(): Promise<void> {
  const target = characters.value.find((c) => c.id === selectedCharacter.value);
  if (!target) return;
  switchingCharacter.value = true;
  status.value = "切換角色中…";
  try {
    setActiveCharacterId(target.id);
    await switchModel(target);
    status.value = `已切換到 ${target.name}`;
    window.setTimeout(() => (status.value = ""), 2000);
  } catch (err) {
    status.value = `切換失敗:${err}`;
  } finally {
    switchingCharacter.value = false;
  }
}

const PREVIEW_LINES = [
  "嗨!這是我現在的聲音,好聽嗎?",
  "今天也要一起加油喔!",
  "欸嘿嘿,你在聽我說話嗎?",
];

function previewVoice(): void {
  const line = PREVIEW_LINES[Math.floor(Math.random() * PREVIEW_LINES.length)];
  // 用面板當下的值試聽(不用先儲存)
  void speak(line, { ...tts.value, enabled: true });
}

async function onClearMemories(): Promise<void> {
  if (!window.confirm("確定要讓她忘掉所有長期記憶嗎?(無法復原)")) return;
  try {
    const n = await clearMemories();
    status.value = `已清除 ${n} 條記憶`;
  } catch (err) {
    status.value = `清除失敗:${err}`;
  }
}

async function onClearHistory(): Promise<void> {
  if (!window.confirm("確定要清空對話紀錄嗎?(無法復原,重啟後生效)")) return;
  try {
    const n = await clearHistory();
    status.value = `已清除 ${n} 則對話`;
  } catch (err) {
    status.value = `清除失敗:${err}`;
  }
}
</script>

<template>
  <div class="panel" @pointerdown.stop>
    <div class="panel-header">
      <span>設定</span>
      <button class="icon-btn" @click="emit('close')">✕</button>
    </div>

    <div v-if="settings" class="panel-body">
      <section v-if="characters.length">
        <h3>角色外觀(Live2D)</h3>
        <label>
          目前角色
          <select
            v-model="selectedCharacter"
            :disabled="switchingCharacter"
            @change="onChangeCharacter"
          >
            <option v-for="c in characters" :key="c.id" :value="c.id">{{ c.name }}</option>
          </select>
        </label>
        <p class="hint">把模型資料夾放進 public\models\,並在 characters.json 加一筆即可新增角色。</p>
      </section>

      <section>
        <h3>角色人設</h3>
        <label>名字<input v-model="settings.persona.name" type="text" /></label>
        <label>
          人設提示詞
          <textarea v-model="settings.persona.system_prompt" rows="4"></textarea>
        </label>
      </section>

      <section>
        <h3>任務路由</h3>
        <div v-for="t in TASKS" :key="t.id" class="route-row">
          <span class="route-label">{{ t.label }}</span>
          <select v-model="settings.routing[t.id].provider">
            <option v-for="p in settings.providers" :key="p.id" :value="p.id">
              {{ p.name }}
            </option>
          </select>
          <input v-model="settings.routing[t.id].model" type="text" placeholder="模型名稱" />
        </div>
        <label class="check">
          <input v-model="settings.fallback_to_local" type="checkbox" />
          雲端失敗時自動降級到本地模型
        </label>
      </section>

      <section>
        <h3>DeepSeek API Key</h3>
        <p class="hint">
          {{ deepseekKeySet ? "✅ 已設定(存於 Windows 認證管理員)" : "尚未設定。留空則僅使用本地模型。" }}
        </p>
        <input
          v-model="deepseekKey"
          type="password"
          :placeholder="deepseekKeySet ? '輸入新 Key 可覆蓋,留空維持不變' : 'sk-…'"
        />
      </section>

      <section>
        <h3>Agent 工具</h3>
        <label class="check">
          <input v-model="settings.agent_enabled" type="checkbox" />
          允許她使用工具(查時間、剪貼簿、開網頁、系統狀態、提醒、記憶)
        </label>
        <p class="hint">
          唯讀工具自動執行;開網頁、雲端讀剪貼簿會先跳出確認卡片問你。
        </p>
      </section>

      <section>
        <h3>自我修改(進階・高風險)</h3>
        <label class="check">
          <input v-model="settings.self_dev_enabled" type="checkbox" />
          允許她讀/改自己的原始碼
        </label>
        <label v-if="settings.self_dev_enabled">
          專案根目錄
          <input
            v-model="settings.self_dev_root"
            type="text"
            placeholder="D:\desktop\desktop-pet-ai"
          />
        </label>
        <p v-if="settings.self_dev_enabled" class="hint warn">
          ⚠️ 每次改檔都會先跳確認卡片、並自動建立 git 還原點;改壞了叫她「還原修改」或自己
          <code>git reset --hard</code>。需要 Agent 工具開啟、且專案是 git 倉庫。改完她會用型別檢查驗證。
        </p>
      </section>

      <section>
        <h3>主動行為</h3>
        <label class="check">
          <input v-model="behavior.proactiveChat" type="checkbox" />
          閒置太久時主動找你聊天
        </label>
        <label v-if="behavior.proactiveChat">
          閒置幾分鐘後主動
          <input v-model.number="behavior.idleMinutes" type="number" min="1" max="120" />
        </label>
        <p class="hint">
          跟她說「○分鐘後提醒我…」她會到時主動跳出來提醒(此功能不受開關影響)。
        </p>
        <label class="check">
          <input v-model="settings.watch_screenshots" type="checkbox" />
          監看截圖資料夾,一截圖就自動幫你看圖評論
        </label>
        <label v-if="settings.watch_screenshots">
          截圖資料夾(留空 = 預設 Pictures\Screenshots)
          <input
            v-model="settings.screenshot_dir"
            type="text"
            placeholder="C:\Users\你\Pictures\Screenshots"
          />
        </label>
        <p v-if="settings.watch_screenshots" class="hint">
          需要視覺模型(看圖路由);儲存後生效。用 Win+PrtScn 截圖會直接存到此資料夾。
        </p>
      </section>

      <section>
        <h3>語音(TTS)</h3>
        <p v-if="!ttsSupported()" class="hint">此環境不支援語音合成。</p>
        <template v-else>
          <label class="check">
            <input v-model="tts.enabled" type="checkbox" />
            朗讀 AI 回覆(托盤「靜音」可暫時關閉)
          </label>
          <label>
            引擎
            <select v-model="tts.engine">
              <option value="auto">自動(Edge 甜美聲線 → Piper → 系統,逐級退回)</option>
              <option value="edge">Edge 神經語音(需網路,最自然)</option>
              <option value="piper">Piper(全本地)</option>
              <option value="system">系統語音(speechSynthesis)</option>
            </select>
          </label>
          <label v-if="tts.engine === 'auto' || tts.engine === 'edge'">
            Edge 聲線
            <select v-model="tts.edgeVoice">
              <option v-for="v in EDGE_VOICES" :key="v.id" :value="v.id">{{ v.label }}</option>
            </select>
          </label>
          <p v-if="speech" class="hint">
            Piper:{{ speech.piper ? `✅ ${speech.piperVoice}` : "未安裝" }}/
            Whisper(語音輸入 Ctrl+Shift+S):{{ speech.whisper ? `✅ ${speech.whisperModel}` : "未安裝" }}
            <br />
            未安裝時執行 scripts\setup-speech.ps1 自動下載(語音輸入退回不可用、朗讀退回系統語音)。
          </p>
          <label v-if="tts.engine === 'system'">
            系統語音
            <select v-model="tts.voice">
              <option value="">自動(優先中文)</option>
              <option v-for="v in voices" :key="v.voiceURI" :value="v.voiceURI">
                {{ v.name }}({{ v.lang }})
              </option>
            </select>
          </label>
          <label>
            語速:{{ tts.rate.toFixed(1) }}
            <input v-model.number="tts.rate" type="range" min="0.5" max="2" step="0.1" />
          </label>
          <label>
            音量:{{ Math.round(tts.volume * 100) }}%
            <input v-model.number="tts.volume" type="range" min="0" max="1" step="0.05" />
          </label>
          <button class="preview" @click="previewVoice">🔊 試聽目前設定</button>
        </template>
      </section>

      <section>
        <h3>記憶</h3>
        <label>
          對話保留輪數
          <input v-model.number="settings.context_turns" type="number" min="1" max="50" />
        </label>
        <p class="hint">
          長期記憶由她自己用工具記下(跟她說「記住…」);對話紀錄會在重啟後自動接上。
        </p>
        <div v-if="isTauri" class="mem-actions">
          <button class="danger" @click="onClearMemories">忘掉所有記憶</button>
          <button class="danger" @click="onClearHistory">清空對話紀錄</button>
        </div>
      </section>
    </div>

    <div class="panel-footer">
      <span class="status">{{ status }}</span>
      <button class="primary" @click="save">儲存</button>
    </div>
  </div>
</template>

<style scoped>
.panel {
  position: absolute;
  inset: 24px 12px;
  display: flex;
  flex-direction: column;
  background: rgba(255, 255, 255, 0.98);
  border-radius: 14px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.35);
  font-size: 13px;
  color: #333;
  overflow: hidden;
}
.panel-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 14px;
  font-weight: 600;
  border-bottom: 1px solid #eee;
}
.icon-btn {
  border: none;
  background: none;
  cursor: pointer;
  font-size: 14px;
}
.panel-body {
  flex: 1;
  overflow-y: auto;
  padding: 10px 14px;
}
section {
  margin-bottom: 16px;
}
h3 {
  font-size: 12px;
  color: #888;
  margin: 0 0 6px;
}
label {
  display: block;
  margin-bottom: 8px;
}
input[type="text"],
input[type="password"],
input[type="number"],
textarea,
select {
  width: 100%;
  box-sizing: border-box;
  margin-top: 4px;
  padding: 6px 8px;
  border: 1px solid #ddd;
  border-radius: 8px;
  font-size: 13px;
  font-family: inherit;
}
input[type="range"] {
  width: 100%;
  margin-top: 4px;
}
.route-row {
  display: grid;
  grid-template-columns: 52px 1fr 1fr;
  gap: 6px;
  align-items: center;
  margin-bottom: 6px;
}
.route-row input,
.route-row select {
  margin-top: 0;
}
.route-label {
  color: #666;
}
.check {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 8px;
}
.check input {
  width: auto;
}
.hint {
  margin: 0 0 6px;
  color: #888;
  font-size: 12px;
}
.hint.warn {
  color: #c66;
}
.hint code {
  background: #f0f0f0;
  padding: 0 4px;
  border-radius: 4px;
}
.panel-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 14px;
  border-top: 1px solid #eee;
}
.status {
  color: #4a9;
  font-size: 12px;
}
.primary {
  border: none;
  background: #5b8def;
  color: #fff;
  padding: 7px 18px;
  border-radius: 16px;
  cursor: pointer;
}
.primary:hover {
  background: #4a7de0;
}
.mem-actions {
  display: flex;
  gap: 8px;
}
.preview {
  border: 1px solid #5b8def;
  background: #fff;
  color: #5b8def;
  padding: 6px 14px;
  border-radius: 14px;
  cursor: pointer;
  font-size: 12px;
}
.preview:hover {
  background: #eef4ff;
}
.danger {
  border: 1px solid #e88;
  background: #fff;
  color: #d55;
  padding: 5px 12px;
  border-radius: 14px;
  cursor: pointer;
  font-size: 12px;
}
.danger:hover {
  background: #fee;
}
</style>
