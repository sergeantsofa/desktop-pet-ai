<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import {
  getSettings,
  saveSettings,
  setApiKey,
  hasApiKey,
  discordConnect,
  spotifyConnect,
  remoteConnect,
  remoteStatus,
  remoteLocalIps,
  authCreateUser,
  authListUsers,
  authDeleteUser,
  authCreateInvite,
  authListInvites,
  authDeleteInvite,
  listModels,
  devListCheckpoints,
  devRestoreCheckpoint,
  backfillEmbeddings,
  getPersonaSummary,
  refreshPersonaSummary,
  clearMemories,
  clearHistory,
  fishStart,
  fishStop,
  gamekbAdd,
  gamekbList,
  gamekbDelete,
  isTauri,
  type Settings,
  type Checkpoint,
  type Invite,
  type GameKnowledge,
} from "../llm/api";
import {
  loadTtsSettings,
  saveTtsSettings,
  listVoices,
  ttsSupported,
  speak,
  EDGE_VOICES,
  DEFAULT_FISH_API_URL,
  type TtsSettings,
} from "../speech/tts";
import { speechStatus, synthesizeFish, type SpeechStatus } from "../speech/native";
import { loadBehavior, saveBehavior, type BehaviorSettings } from "../behavior";
import {
  loadCharacters,
  getActiveCharacterId,
  setActiveCharacterId,
  type Character,
} from "../live2d/characters";
import {
  loadCharScale,
  saveCharScale,
  getWindowSize,
  setWindowSize,
  loadBubbleStyle,
  saveBubbleStyle,
  BUBBLE_STYLES,
  CHAR_SCALE_MIN,
  CHAR_SCALE_MAX,
  WIN_MIN,
  WIN_MAX,
} from "../appearance";
import SpeechBubble from "../chat/SpeechBubble.vue";
import {
  checkForUpdateDetailed,
  downloadAndInstall,
  currentVersion,
  formatBytes,
  getLastCheckAt,
  loadUpdateSettings,
  saveUpdateSettings,
  describeUpdateError,
  openReleasesPage,
  backupAppData,
  listBackups,
  openBackupsFolder,
  type UpdateState,
  type UpdateSettings,
  type BackupInfo,
} from "../updater";

/* 設定視窗是獨立的 OS 視窗,改外觀要用 Tauri 事件通知桌寵本體(main)即時套用 */
async function petEmit(event: string, payload: unknown): Promise<void> {
  if (!isTauri) return;
  try {
    const { emit } = await import("@tauri-apps/api/event");
    await emit(event, payload);
  } catch {
    /* 送不出去就算了,儲存的值主視窗下次也會讀到 */
  }
}

function closeWindow(): void {
  if (!isTauri) return;
  void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
    getCurrentWindow().close()
  );
}

/* ---------- 分頁分類 ---------- */
const TABS = [
  { id: "about", label: "關於・更新", icon: "🔄" },
  { id: "appearance", label: "外觀", icon: "🎨" },
  { id: "persona", label: "人設・大腦", icon: "🧠" },
  { id: "voice", label: "語音", icon: "🔊" },
  { id: "behavior", label: "行為", icon: "✨" },
  { id: "memory", label: "記憶", icon: "📒" },
  { id: "integrations", label: "連動", icon: "🔗" },
  { id: "advanced", label: "進階", icon: "🛠️" },
];
const activeTab = ref("appearance");

const settings = ref<Settings | null>(null);
const deepseekKey = ref("");
const deepseekKeySet = ref(false);
const discordToken = ref("");
const discordTokenSet = ref(false);
const discordChannelsText = ref("");
const discordStatus = ref("");
const spotifyLinked = ref(false);
const spotifyStatus = ref("");
const remoteStatusText = ref("");
const remoteUrls = ref<string[]>([]);
const remoteLocalIpList = ref<string[]>([]);
const webUsers = ref<string[]>([]);
const newWebUser = ref("");
const newWebPass = ref("");
const webUserStatus = ref("");
const invites = ref<Invite[]>([]);
const inviteStatus = ref("");
/** provider id → 該 provider 可用模型清單(下拉用);空 = 退回手動輸入 */
const modelsByProvider = ref<Record<string, string[]>>({});
/** task id → 是否切到「手動輸入模型」模式 */
const customModelRows = ref<Record<string, boolean>>({});
const CUSTOM_MODEL = "__custom__";

/** DeepSeek 已知模型靜態清單(當 API 抓不到時作為 fallback) */
const DEEPSEEK_MODELS = [
  "deepseek-chat",
  "deepseek-reasoner",
  "deepseek-v3",
  "deepseek-v2.5",
  "deepseek-v4-flash",
  "deepseek-coder-v2",
  "deepseek-r1",
  "deepseek-r1-distill-qwen-7b",
  "deepseek-r1-distill-qwen-14b",
  "deepseek-r1-distill-qwen-32b",
  "deepseek-r1-distill-llama-70b",
];
const checkpoints = ref<Checkpoint[]>([]);
const checkpointStatus = ref("");
const personaSummary = ref<string | null>(null);
const memStatus = ref("");
const status = ref("");
const tts = ref<TtsSettings>(loadTtsSettings());
const voices = ref<SpeechSynthesisVoice[]>([]);
const speech = ref<SpeechStatus | null>(null);
const behavior = ref<BehaviorSettings>(loadBehavior());
const characters = ref<Character[]>([]);
const selectedCharacter = ref("");
const charScale = ref(loadCharScale());
const winW = ref(0);
const winH = ref(0);
const bubbleStyle = ref(loadBubbleStyle());
const bubblePreviewText = "嗨~這是我說話的樣子!";

/* ---------- 關於 / 更新 ---------- */
const appVersion = ref("");
const upd = ref<UpdateState>({ kind: "idle" });
const updateCfg = ref<UpdateSettings>(loadUpdateSettings());
const lastCheck = ref(getLastCheckAt());
let pendingUpdate: Awaited<ReturnType<typeof checkForUpdateDetailed>> = null;

const updateStatusText = computed(() => {
  const s = upd.value;
  switch (s.kind) {
    case "idle":
      return "還沒檢查過,按下面的按鈕看看有沒有新版。";
    case "checking":
      return "檢查中…";
    case "up-to-date":
      return `✅ 已是最新版本(v${s.current})`;
    case "available":
      return `🎉 新版本 v${s.version} 可用(目前 v${s.current})`;
    case "downloading":
      return "下載更新中…";
    case "backing-up":
      return "更新前備份資料中…";
    case "installing":
      return "安裝中,即將自動重啟…";
    case "done":
      return "✅ 更新完成,正在重啟…";
    case "error":
      return s.message;
  }
  return "";
});

const busy = computed(
  () =>
    upd.value.kind === "checking" ||
    upd.value.kind === "downloading" ||
    upd.value.kind === "backing-up" ||
    upd.value.kind === "installing"
);

const downloadPercent = computed(() => {
  const s = upd.value;
  if (s.kind !== "downloading" || !s.total) return null;
  return Math.min(100, Math.round((s.downloaded / s.total) * 100));
});

const lastCheckText = computed(() => {
  if (!lastCheck.value) return "從未檢查";
  const mins = Math.floor((Date.now() - lastCheck.value) / 60000);
  if (mins < 1) return "剛剛";
  if (mins < 60) return `${mins} 分鐘前`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs} 小時前`;
  return `${Math.floor(hrs / 24)} 天前`;
});

/** 開發模式(非 Tauri)沒有 getVersion,直接顯示 dev */
async function initUpdate(): Promise<void> {
  updateCfg.value = loadUpdateSettings();
  lastCheck.value = getLastCheckAt();
  appVersion.value = isTauri ? await currentVersion() : "dev(開發模式)";
  if (isTauri) await refreshBackups();
}

function onUpdateCfgChange(): void {
  saveUpdateSettings(updateCfg.value);
}

async function onCheckUpdate(): Promise<void> {
  if (busy.value) return;
  try {
    pendingUpdate = await checkForUpdateDetailed((s) => {
      upd.value = s;
    });
    lastCheck.value = Date.now();
  } catch (err) {
    upd.value = { kind: "error", message: describeUpdateError(err) };
  }
}

async function onInstallUpdate(): Promise<void> {
  if (!pendingUpdate) return;
  try {
    // 更新前保險:先把記憶/設定/模型快照一份(可在下面關掉)
    if (updateCfg.value.backupBeforeUpdate) {
      upd.value = { kind: "backing-up" };
      try {
        await backupAppData();
        await refreshBackups();
      } catch {
        /* 備份失敗不擋更新,但上面會留狀態 */
      }
    }
    await downloadAndInstall(pendingUpdate, (s) => {
      upd.value = s;
    });
  } catch (err) {
    upd.value = { kind: "error", message: describeUpdateError(err) };
  }
}

async function onOpenReleases(): Promise<void> {
  await openReleasesPage();
}

/* ---------- 更新前備份 ---------- */
const backups = ref<BackupInfo[]>([]);
const backupStatus = ref("");
const backingUp = ref(false);

function fmtLabel(label: string): string {
  // 2026-10-07_153000 → 2026-10-07 15:30
  const m = label.match(/^(\d{4}-\d{2}-\d{2})_(\d{2})(\d{2})(\d{2})/);
  return m ? `${m[1]} ${m[2]}:${m[3]}` : label;
}

async function refreshBackups(): Promise<void> {
  backups.value = await listBackups();
}

async function onBackupNow(): Promise<void> {
  if (backingUp.value) return;
  backingUp.value = true;
  backupStatus.value = "備份中…";
  try {
    const r = await backupAppData();
    backupStatus.value = r.memory_included
      ? `✅ 已備份 ${r.files} 個檔(${formatBytes(r.bytes)}),保留最近 ${r.kept} 份`
      : `⚠️ 備份完成(${r.files} 個檔),但沒找到記憶資料庫 memory.db`;
    await refreshBackups();
  } catch (err) {
    backupStatus.value = `備份失敗:${err instanceof Error ? err.message : String(err)}`;
  } finally {
    backingUp.value = false;
  }
}
const TASKS: Array<{ id: string; label: string }> = [
  { id: "chat", label: "閒聊" },
  { id: "coder", label: "寫程式" },
  { id: "reasoner", label: "推理" },
  { id: "vision", label: "看圖" },
  { id: "gamekb", label: "遊戲問答" },
];

/* 遊戲知識庫 */
const gameKb = ref<GameKnowledge[]>([]);
const newGameFact = ref("");
const gameKbStatus = ref("");

async function loadGameKb(): Promise<void> {
  gameKb.value = await gamekbList();
}
async function onAddGameFact(): Promise<void> {
  const t = newGameFact.value.trim();
  if (!t) return;
  try {
    await gamekbAdd(t);
    newGameFact.value = "";
    await loadGameKb();
    gameKbStatus.value = "已教給她了!";
  } catch (err) {
    gameKbStatus.value = `失敗:${err}`;
  }
}
async function onDeleteGameFact(id: number): Promise<void> {
  try {
    await gamekbDelete(id);
    await loadGameKb();
  } catch (err) {
    gameKbStatus.value = `刪除失敗:${err}`;
  }
}

/** 舊設定檔可能缺某些任務路由,補上預設以免 UI 綁定到 undefined */
const ROUTE_DEFAULTS: Record<string, { provider: string; model: string }> = {
  chat: { provider: "ollama", model: "qwen2.5:7b" },
  coder: { provider: "ollama", model: "qwen2.5-coder:7b" },
  reasoner: { provider: "ollama", model: "deepseek-r1:7b" },
  vision: { provider: "ollama", model: "qwen2.5vl:3b" },
  gamekb: { provider: "deepseek", model: "deepseek-v4-flash" },
};

function ensureRoutes(s: Settings): void {
  for (const t of TASKS) {
    if (!s.routing[t.id]) s.routing[t.id] = { ...ROUTE_DEFAULTS[t.id] };
  }
}

/** 抓某 provider 的模型清單(快取一次);DeepSeek 抓不到時退回靜態清單 */
async function loadModels(providerId: string): Promise<void> {
  if (!providerId || modelsByProvider.value[providerId]) return;
  let list = await listModels(providerId);
  if (list.length === 0 && providerId === "deepseek") {
    list = DEEPSEEK_MODELS;
  }
  modelsByProvider.value = { ...modelsByProvider.value, [providerId]: list };
}

/** 某 task 的模型下拉選項(含目前值,確保顯示正確) */
function modelOptions(taskId: string): string[] {
  if (!settings.value) return [];
  const list = modelsByProvider.value[settings.value.routing[taskId].provider] ?? [];
  const cur = settings.value.routing[taskId].model;
  return cur && !list.includes(cur) ? [cur, ...list] : list;
}

/** 切換 provider:重抓清單、退出手動模式 */
function onProviderChange(taskId: string): void {
  customModelRows.value[taskId] = false;
  if (settings.value) void loadModels(settings.value.routing[taskId].provider);
}

/** 模型下拉選擇(選到「自己輸入」就切文字模式) */
function onModelSelect(taskId: string, val: string): void {
  if (val === CUSTOM_MODEL) {
    customModelRows.value[taskId] = true;
  } else if (settings.value) {
    settings.value.routing[taskId].model = val;
  }
}

onMounted(async () => {
  characters.value = await loadCharacters();
  selectedCharacter.value = await getActiveCharacterId();
  const ws = await getWindowSize();
  if (ws) {
    winW.value = ws.w;
    winH.value = ws.h;
  }
  if (ttsSupported()) {
    // 中文語音排前面,挑選方便
    const all = await listVoices();
    voices.value = [
      ...all.filter((v) => /^zh/i.test(v.lang)),
      ...all.filter((v) => !/^zh/i.test(v.lang)),
    ];
  }
  if (isTauri) speech.value = await speechStatus();
  void initUpdate();
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
      self_dev_auto_verify: true,
      discord_enabled: false,
      discord_channels: [],
      spotify_enabled: false,
      spotify_client_id: "",
      remote_enabled: false,
      remote_host: "",
      remote_port: 8765,
      embed_model: "nomic-embed-text",
      fish_autostart: false,
      fish_launch_cmd: "",
      fish_cwd: "",
      fish_api_base: "http://127.0.0.1:8080",
      game_kb_enabled: false,
    };
    ensureRoutes(settings.value);
    return;
  }
  const loaded = await getSettings();
  ensureRoutes(loaded);
  settings.value = loaded;
  for (const p of loaded.providers) void loadModels(p.id); // 預抓模型清單給下拉
  deepseekKeySet.value = await hasApiKey("deepseek");
  discordTokenSet.value = await hasApiKey("discord");
  discordChannelsText.value = (loaded.discord_channels ?? []).join("\n");
  spotifyLinked.value = await hasApiKey("spotify");
  remoteLocalIpList.value = await remoteLocalIps();
  const rs = await remoteStatus();
  remoteUrls.value = rs.urls;
  if (rs.running) remoteStatusText.value = `✅ 執行中(綁定 ${rs.bound})`;
  personaSummary.value = await getPersonaSummary();
  webUsers.value = await authListUsers();
  invites.value = await authListInvites();
  gameKb.value = await gamekbList();
});

async function save(): Promise<void> {
  if (!settings.value) return;
  try {
    saveTtsSettings(tts.value);
    saveBehavior(behavior.value);
    saveCharScale(charScale.value);
    saveBubbleStyle(bubbleStyle.value);
    // 確保桌寵本體也同步外觀(以防使用者沒觸發即時事件)
    await petEmit("pet:set-scale", charScale.value);
    await petEmit("pet:set-bubble", bubbleStyle.value);
    if (isTauri) {
      // 把 Discord 頻道輸入框(每行一個 ID)同步進設定
      settings.value.discord_channels = parseChannels(discordChannelsText.value);
      await saveSettings(settings.value);
      if (deepseekKey.value.trim()) {
        await setApiKey("deepseek", deepseekKey.value);
        deepseekKey.value = "";
        deepseekKeySet.value = true;
      }
      if (discordToken.value.trim()) {
        await setApiKey("discord", discordToken.value);
        discordToken.value = "";
        discordTokenSet.value = true;
      }
    }
    status.value = "已儲存!";
    window.setTimeout(() => (status.value = ""), 2000);
  } catch (err) {
    status.value = `儲存失敗:${err}`;
  }
}

/** 把多行/逗號分隔的頻道輸入,清成純數字 ID 陣列 */
function parseChannels(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter((s) => /^\d+$/.test(s));
}

/** 先存設定+Token,再叫 Rust 嘗試連線 Discord */
async function onDiscordConnect(): Promise<void> {
  discordStatus.value = "連線中…";
  try {
    await save();
    discordStatus.value = await discordConnect();
  } catch (err) {
    discordStatus.value = `失敗:${err}`;
  }
}

/** 先存 Client ID,再開瀏覽器授權連結 Spotify */
async function onSpotifyConnect(): Promise<void> {
  spotifyStatus.value = "開啟瀏覽器授權中…請在跳出的分頁登入並同意。";
  try {
    await save();
    spotifyStatus.value = await spotifyConnect();
    spotifyLinked.value = await hasApiKey("spotify");
  } catch (err) {
    spotifyStatus.value = `失敗:${err}`;
  }
}

/** 先存設定(含 IP/埠),再啟動/重啟區網遠端聊天伺服器 */
async function onRemoteConnect(): Promise<void> {
  remoteStatusText.value = "啟動中…";
  try {
    await save();
    remoteStatusText.value = await remoteConnect();
    const rs = await remoteStatus();
    remoteUrls.value = rs.urls;
  } catch (err) {
    remoteStatusText.value = `失敗:${err}`;
  }
}

/** 新增一個網頁登入帳號 */
async function onAddWebUser(): Promise<void> {
  const u = newWebUser.value.trim();
  if (!u || !newWebPass.value) {
    webUserStatus.value = "帳號與密碼都要填";
    return;
  }
  try {
    await authCreateUser(u, newWebPass.value);
    newWebUser.value = "";
    newWebPass.value = "";
    webUsers.value = await authListUsers();
    webUserStatus.value = `已新增帳號:${u}`;
  } catch (err) {
    webUserStatus.value = `新增失敗:${err}`;
  }
}

/** 刪除網頁帳號(連同其私密記憶) */
async function onDeleteWebUser(name: string): Promise<void> {
  if (!window.confirm(`刪除帳號「${name}」?(會連同他的私密記憶一起刪掉,無法復原)`)) return;
  try {
    await authDeleteUser(name);
    webUsers.value = await authListUsers();
    webUserStatus.value = `已刪除帳號:${name}`;
  } catch (err) {
    webUserStatus.value = `刪除失敗:${err}`;
  }
}

/** 產生一組一次性邀請碼 */
async function onCreateInvite(): Promise<void> {
  try {
    const code = await authCreateInvite();
    invites.value = await authListInvites();
    inviteStatus.value = `新邀請碼:${code}(把它給要註冊的人)`;
  } catch (err) {
    inviteStatus.value = `產生失敗:${err}`;
  }
}

/** 刪除邀請碼 */
async function onDeleteInvite(code: string): Promise<void> {
  try {
    await authDeleteInvite(code);
    invites.value = await authListInvites();
  } catch (err) {
    inviteStatus.value = `刪除失敗:${err}`;
  }
}

/** 載入自我修改的還原點(時間軸) */
async function loadCheckpoints(): Promise<void> {
  checkpointStatus.value = "載入中…";
  try {
    checkpoints.value = await devListCheckpoints();
    checkpointStatus.value = checkpoints.value.length ? "" : "目前沒有還原點(或專案不是 git 倉庫)。";
  } catch (err) {
    checkpointStatus.value = `載入失敗:${err}`;
  }
}

/** 還原到某個還原點(破壞性:會丟掉這之後的未保存變更,所以先確認) */
async function onRestoreCheckpoint(cp: Checkpoint): Promise<void> {
  const ok = window.confirm(
    `確定要還原到這個還原點嗎?\n\n${cp.date}\n${cp.note}\n\n` +
      `這會把整個專案還原到那個時間點(還原前的現狀會自動再存一個快照,可以再還原回來)。`
  );
  if (!ok) return;
  checkpointStatus.value = "還原中…";
  try {
    checkpointStatus.value = await devRestoreCheckpoint(cp.sha);
    await loadCheckpoints();
  } catch (err) {
    checkpointStatus.value = `還原失敗:${err}`;
  }
}

/** 角色大小滑桿:即存 + 即時通知桌寵本體套用 */
function onCharScaleInput(): void {
  saveCharScale(charScale.value);
  void petEmit("pet:set-scale", charScale.value);
}

/** 泡泡樣式變更:即存 + 即時通知桌寵本體 */
function onBubbleChange(): void {
  saveBubbleStyle(bubbleStyle.value);
  void petEmit("pet:set-bubble", bubbleStyle.value);
}

/** 視窗寬高輸入:套用到角色視窗(main),不影響本設定視窗 */
async function onWindowSizeInput(): Promise<void> {
  if (!winW.value || !winH.value) return;
  try {
    await setWindowSize(winW.value, winH.value);
    // 後端可能因上下限夾過值,回讀一次讓欄位顯示真實尺寸
    const ws = await getWindowSize();
    if (ws) {
      winW.value = ws.w;
      winH.value = ws.h;
    }
  } catch (err) {
    status.value = `調整視窗大小失敗:${err}`;
  }
}

/** 切換角色:記住選擇 + 通知桌寵本體換模型 */
function onChangeCharacter(): void {
  const target = characters.value.find((c) => c.id === selectedCharacter.value);
  if (!target) return;
  setActiveCharacterId(target.id);
  void petEmit("pet:switch-character", target.id);
  status.value = `已切換到 ${target.name}`;
  window.setTimeout(() => (status.value = ""), 2000);
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

/* ---------- Fish Speech 自動啟動 / 診斷 ---------- */
const fishStatus = ref("");

/** 啟動本地 Fish API server(先存設定,讓後端讀到指令/工作目錄) */
async function onFishStart(): Promise<void> {
  fishStatus.value = "啟動中…";
  try {
    await save();
    fishStatus.value = await fishStart();
  } catch (err) {
    fishStatus.value = `失敗:${err}`;
  }
}

/** 停止由桌寵啟動的 Fish */
async function onFishStop(): Promise<void> {
  try {
    fishStatus.value = await fishStop();
  } catch (err) {
    fishStatus.value = `失敗:${err}`;
  }
}

/** 診斷:直接打 Fish API 合成一小段,把真實結果/錯誤顯示出來(不會像朗讀那樣安靜退回) */
async function onFishTest(): Promise<void> {
  fishStatus.value = "🩺 測試中…(打 Fish API)";
  try {
    const buf = await synthesizeFish(
      "你好,這是 Fish Speech 測試。",
      tts.value.fishApiUrl,
      tts.value.fishRefAudio,
      tts.value.fishRefText
    );
    if (!buf || buf.byteLength === 0) {
      fishStatus.value = "⚠️ 連到了但回傳空音訊(檢查參考音訊/參數)";
      return;
    }
    fishStatus.value = `✅ Fish 正常!收到 ${(buf.byteLength / 1024).toFixed(1)} KB 音訊,播放中…`;
    const url = URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
    const audio = new Audio(url);
    audio.onended = () => URL.revokeObjectURL(url);
    void audio.play();
  } catch (err) {
    // tts_fish 會帶出明確錯誤:連線失敗(沒開/端點錯)、HTTP 狀態碼、空音訊…
    fishStatus.value = `❌ ${err}`;
  }
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

/** 把舊記憶補算語意向量 */
async function onBackfill(): Promise<void> {
  memStatus.value = "回填中…(需要 Ollama embedding 模型)";
  try {
    const n = await backfillEmbeddings();
    memStatus.value = n > 0 ? `已補上 ${n} 條記憶的語意向量` : "沒有需要回填的記憶(或 embedding 模型不可用)";
  } catch (err) {
    memStatus.value = `回填失敗:${err}`;
  }
}

/** 重新整理「她眼中的你」人格摘要 */
async function onRefreshPersona(): Promise<void> {
  memStatus.value = "重新整理人格印象中…";
  try {
    personaSummary.value = await refreshPersonaSummary();
    memStatus.value = "已更新人格印象";
  } catch (err) {
    memStatus.value = `更新失敗:${err}`;
  }
}
</script>

<template>
  <div class="settings-root">
    <header class="win-header">
      <span>設定</span>
      <button class="icon-btn" title="關閉" @click="closeWindow">✕</button>
    </header>

    <div class="win-main">
      <nav class="tabs">
        <button
          v-for="t in TABS"
          :key="t.id"
          class="tab-btn"
          :class="{ active: activeTab === t.id }"
          @click="activeTab = t.id"
        >
          <span class="tab-ico">{{ t.icon }}</span>{{ t.label }}
        </button>
      </nav>

      <div class="tab-content">
        <template v-if="settings">
          <!-- ===== 關於 / 更新 ===== -->
          <div v-show="activeTab === 'about'">
            <section>
              <h3>版本資訊</h3>
              <ul class="ver-list">
                <li><span>目前版本</span><code>v{{ appVersion || "…" }}</code></li>
                <li><span>上次檢查</span><span class="dim">{{ lastCheckText }}</span></li>
              </ul>
              <button class="action" :disabled="busy" @click="onCheckUpdate">
                <template v-if="upd.kind === 'checking'">檢查中…</template>
                <template v-else>🔍 檢查更新</template>
              </button>
              <p
                class="hint"
                :class="{
                  ok: upd.kind === 'up-to-date' || upd.kind === 'done',
                  warn: upd.kind === 'error',
                }"
              >
                {{ updateStatusText }}
              </p>

              <div v-if="upd.kind === 'downloading'" class="upd-progress">
                <div class="upd-bar">
                  <div class="upd-bar-fill" :style="{ width: (downloadPercent ?? 0) + '%' }" />
                </div>
                <span class="upd-bytes">
                  {{ formatBytes(upd.downloaded)
                  }}<template v-if="upd.total"> / {{ formatBytes(upd.total) }}</template>
                  <template v-if="downloadPercent !== null"> · {{ downloadPercent }}%</template>
                </span>
              </div>

              <div v-if="upd.kind === 'available'" class="upd-card">
                <b>🎉 v{{ upd.version }} 可以更新了</b>
                <p v-if="upd.date" class="dim">發布於 {{ upd.date }}</p>
                <pre v-if="upd.notes" class="upd-notes">{{ upd.notes }}</pre>
                <div class="upd-actions">
                  <button class="primary" @click="onInstallUpdate">立即更新</button>
                  <button class="action" @click="upd = { kind: 'idle' }">稍後</button>
                </div>
              </div>
            </section>

            <section>
              <h3>自動更新</h3>
              <label class="check">
                <input
                  v-model="updateCfg.autoCheck"
                  type="checkbox"
                  @change="onUpdateCfgChange"
                />
                啟動時自動檢查更新
              </label>
              <label class="check">
                <input
                  v-model="updateCfg.autoDownload"
                  type="checkbox"
                  @change="onUpdateCfgChange"
                />
                發現新版就自動下載、安裝並重啟(全自動)
              </label>
              <label>
                更新代理(連不上 GitHub 時填,例如 http://127.0.0.1:7890)
                <input
                  v-model="updateCfg.proxy"
                  type="text"
                  placeholder="留空 = 直連"
                  @change="onUpdateCfgChange"
                />
              </label>
              <p class="hint">
                自動檢查有 24 小時節流,不會一直打 GitHub。關掉「全自動」就只會冒泡泡提醒,由你按更新。
              </p>
              <button class="action" @click="onOpenReleases">📦 查看所有版本</button>
            </section>

            <section>
              <h3>更新前備份(保險)</h3>
              <label class="check">
                <input
                  v-model="updateCfg.backupBeforeUpdate"
                  type="checkbox"
                  @change="onUpdateCfgChange"
                />
                更新前自動備份記憶 / 設定 / 模型
              </label>
              <p class="hint">
                更新前會把整個資料夾快照一份到 <code>backups\</code>,只保留最近 3 份。
                新版有問題時,把備份內容覆蓋回去就能回到原狀。
              </p>
              <div class="upd-actions">
                <button class="action" :disabled="backingUp" @click="onBackupNow">
                  <template v-if="backingUp">備份中…</template>
                  <template v-else>💾 立即備份</template>
                </button>
                <button class="action" @click="openBackupsFolder">📂 開啟資料夾</button>
              </div>
              <p v-if="backupStatus" class="hint ok">{{ backupStatus }}</p>
              <ul v-if="backups.length" class="ver-list backup-list">
                <li v-for="b in backups" :key="b.label">
                  <span>{{ fmtLabel(b.label) }}</span>
                  <span class="dim">{{ formatBytes(b.bytes) }}</span>
                </li>
              </ul>
              <p v-else class="hint">目前還沒有任何備份。</p>
            </section>
          </div>

          <!-- ===== 外觀 ===== -->
          <div v-show="activeTab === 'appearance'">
            <section v-if="characters.length">
              <h3>角色外觀(Live2D)</h3>
              <label>
                目前角色
                <select v-model="selectedCharacter" @change="onChangeCharacter">
                  <option v-for="c in characters" :key="c.id" :value="c.id">{{ c.name }}</option>
                </select>
              </label>
              <p class="hint">把模型資料夾放進 public\models\,並在 characters.json 加一筆即可新增角色。</p>
            </section>

            <section>
              <h3>角色大小</h3>
              <label>
                角色縮放:{{ Math.round(charScale * 100) }}%
                <input
                  v-model.number="charScale"
                  type="range"
                  :min="CHAR_SCALE_MIN"
                  :max="CHAR_SCALE_MAX"
                  step="0.05"
                  @input="onCharScaleInput"
                />
              </label>
              <p class="hint">在角色視窗內把角色放大/縮小(不改變視窗大小)。</p>
            </section>

            <section v-if="isTauri">
              <h3>角色視窗大小</h3>
              <div class="size-row">
                <label>
                  寬(px)
                  <input
                    v-model.number="winW"
                    type="number"
                    :min="WIN_MIN.w"
                    :max="WIN_MAX.w"
                    @change="onWindowSizeInput"
                  />
                </label>
                <label>
                  高(px)
                  <input
                    v-model.number="winH"
                    type="number"
                    :min="WIN_MIN.h"
                    :max="WIN_MAX.h"
                    @change="onWindowSizeInput"
                  />
                </label>
              </div>
              <p class="hint">這是調「桌寵角色視窗」的大小(本設定視窗不受影響;視窗變大角色也會等比變大)。</p>
            </section>

            <section>
              <h3>對話泡泡樣式</h3>
              <label>
                樣式
                <select v-model="bubbleStyle" @change="onBubbleChange">
                  <option v-for="s in BUBBLE_STYLES" :key="s.id" :value="s.id">{{ s.label }}</option>
                </select>
              </label>
              <div class="bubble-preview">
                <SpeechBubble :text="bubblePreviewText" :variant="bubbleStyle" />
              </div>
              <p class="hint">選好即時套用,她下次說話就會用這個樣式。</p>
            </section>
          </div>

          <!-- ===== 人設・大腦 ===== -->
          <div v-show="activeTab === 'persona'">
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
                <select v-model="settings.routing[t.id].provider" @change="onProviderChange(t.id)">
                  <option v-for="p in settings.providers" :key="p.id" :value="p.id">
                    {{ p.name }}
                  </option>
                </select>
                <!-- 有抓到模型清單就用下拉(可挑已安裝的);否則/選「自己輸入」退回文字框 -->
                <select
                  v-if="!customModelRows[t.id] && modelOptions(t.id).length"
                  :value="settings.routing[t.id].model"
                  @change="onModelSelect(t.id, ($event.target as HTMLSelectElement).value)"
                >
                  <option v-for="m in modelOptions(t.id)" :key="m" :value="m">{{ m }}</option>
                  <option :value="CUSTOM_MODEL">✏️ 自己輸入…</option>
                </select>
                <input v-else v-model="settings.routing[t.id].model" type="text" placeholder="模型名稱" />
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
          </div>

          <!-- ===== 語音 ===== -->
          <div v-show="activeTab === 'voice'">
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
                    <option value="auto">自動(Fish Speech 本地 AI → Edge 甜美聲線 → Piper → 系統,逐級退回)</option>
                    <option value="fish">Fish Speech(本地 AI 語音,需啟動 API 服務)</option>
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
                <!-- Fish Speech 設定 -->
                <template v-if="tts.engine === 'auto' || tts.engine === 'fish'">
                  <details class="fish-settings">
                    <summary>⚙️ Fish Speech 進階設定</summary>
                    <label>
                      API 位址
                      <input v-model="tts.fishApiUrl" type="url" placeholder="http://127.0.0.1:8080/v1/tts"
                             @blur="tts.fishApiUrl = tts.fishApiUrl || DEFAULT_FISH_API_URL" />
                    </label>
                    <label>
                      參考音訊路徑(台灣腔範本 WAV)
                      <input v-model="tts.fishRefAudio" type="text" placeholder="留空 = 使用預設聲線" />
                    </label>
                    <label>
                      參考文本(對應參考音訊的內容)
                      <textarea v-model="tts.fishRefText" rows="2"
                                placeholder="請輸入參考音訊中說的文字內容，留空 = Fish Speech 自動辨識" />
                    </label>

                    <hr class="fish-divider" />
                    <label class="check">
                      <input v-model="settings.fish_autostart" type="checkbox" />
                      桌寵啟動時自動把 Fish API server 跑起來(結束時關掉)
                    </label>
                    <label>
                      Fish 啟動指令
                      <input
                        v-model="settings.fish_launch_cmd"
                        type="text"
                        placeholder="例:python -m tools.api_server --listen 127.0.0.1:8080"
                      />
                    </label>
                    <label>
                      工作目錄(fish-speech 專案根)
                      <input
                        v-model="settings.fish_cwd"
                        type="text"
                        placeholder="例:D:\fish-speech(留空 = 不切目錄)"
                      />
                    </label>
                    <label>
                      API 根位址(用來判斷是否已在執行)
                      <input v-model="settings.fish_api_base" type="text" placeholder="http://127.0.0.1:8080" />
                    </label>
                    <p class="hint warn">
                      ⚠️ 指令要跑「有 <code>/v1/tts</code> 端點的 API server」(不是 <code>fish_speech.webui</code> 那個網頁 UI),
                      否則桌寵會連不上而安靜退回 Edge。若用 conda 請寫成 <code>conda run -n 你的環境 python -m …</code>。
                    </p>
                    <div class="mem-actions">
                      <button class="action" @click="onFishStart">▶️ 啟動 Fish</button>
                      <button class="action" @click="onFishStop">⏹ 停止</button>
                      <button class="preview" @click="onFishTest">🩺 測試 Fish</button>
                    </div>
                    <p v-if="fishStatus" class="hint">{{ fishStatus }}</p>
                  </details>
                </template>
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
          </div>

          <!-- ===== 行為 ===== -->
          <div v-show="activeTab === 'behavior'">
            <section>
              <h3>主動行為</h3>
              <label class="check">
                <input v-model="behavior.hourlyChime" type="checkbox" />
                每整點報時
              </label>
              <label v-if="behavior.hourlyChime">
                報時風格
                <select v-model="behavior.chimeStyle">
                  <option value="gentle">溫柔(輕聲提醒)</option>
                  <option value="playful">俏皮(活潑跳跳)</option>
                  <option value="minimal">簡潔(只報時間)</option>
                </select>
              </label>
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
              <label>
                連續對話:講完靜音幾秒就回答(Ctrl+Shift+D)
                <input
                  v-model.number="behavior.converseSilenceSec"
                  type="number"
                  min="0.5"
                  max="10"
                  step="0.5"
                />
              </label>
              <p class="hint">
                按 <code>Ctrl+Shift+D</code> 開始/結束連續對話;你停頓超過這個秒數她就回答,
                你一開口她會立刻停下來等你說完。需要 Whisper 語音輸入(見語音分頁)。
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
              <h3>🎮 遊戲知識庫模式</h3>
              <label class="check">
                <input v-model="settings.game_kb_enabled" type="checkbox" />
                開啟後:她只用「你教過的遊戲知識」回答,不在知識庫裡就說不知道(不亂編)
              </label>
              <p class="hint">
                教法:① 對話時打「<code>教:○○○</code>」隨手教一條;② 下面手動新增。
                問答模型在「人設・大腦 → 任務路由 → 遊戲問答」設定(預設雲端 DeepSeek,需 Key)。
              </p>
              <div class="mem-actions">
                <input
                  v-model="newGameFact"
                  type="text"
                  placeholder="輸入一條遊戲知識,例如:最終王的弱點是火屬性"
                  @keyup.enter="onAddGameFact"
                />
                <button class="action" @click="onAddGameFact">教她</button>
              </div>
              <p v-if="gameKbStatus" class="hint">{{ gameKbStatus }}</p>
              <p class="hint">目前知識庫:{{ gameKb.length }} 條</p>
              <ul v-if="gameKb.length" class="checkpoints">
                <li v-for="g in gameKb" :key="g.id">
                  <div class="cp-info"><span class="cp-note">{{ g.content }}</span></div>
                  <button class="cp-restore" @click="onDeleteGameFact(g.id)">刪除</button>
                </li>
              </ul>
            </section>
          </div>

          <!-- ===== 記憶 ===== -->
          <div v-show="activeTab === 'memory'">
            <section>
              <h3>記憶</h3>
              <label>
                對話保留輪數
                <input v-model.number="settings.context_turns" type="number" min="1" max="50" />
              </label>
              <label>
                語意記憶 embedding 模型(Ollama)
                <input
                  v-model="settings.embed_model"
                  type="text"
                  placeholder="nomic-embed-text(留空 = 停用語意,退回關鍵字)"
                />
              </label>
              <p class="hint">
                長期記憶由她自己用工具記下(跟她說「記住…」);對話紀錄會在重啟後自動接上。
                「翻記憶」會用<b>語意搜尋</b>(找意思相近的、不只字面),需先
                <code>ollama pull {{ settings.embed_model || "nomic-embed-text" }}</code>;
                抓不到模型時自動退回關鍵字,不會壞。重要的記憶會被優先想起。
              </p>
              <template v-if="isTauri">
                <p class="hint">
                  <b>她眼中的你</b>(由記憶蒸餾,會隨相處更新):
                </p>
                <p class="persona-summary">{{ personaSummary || "(還沒有印象~多跟她聊聊、讓她記點事)" }}</p>
                <div class="mem-actions">
                  <button class="action" @click="onRefreshPersona">🪄 重新整理人格印象</button>
                  <button class="action" @click="onBackfill">🔁 回填舊記憶向量</button>
                </div>
                <p v-if="memStatus" class="hint">{{ memStatus }}</p>
                <div class="mem-actions">
                  <button class="danger" @click="onClearMemories">忘掉所有記憶</button>
                  <button class="danger" @click="onClearHistory">清空對話紀錄</button>
                </div>
              </template>
            </section>
          </div>

          <!-- ===== 連動 ===== -->
          <div v-show="activeTab === 'integrations'">
            <section>
              <h3>Discord 串接</h3>
              <label class="check">
                <input v-model="settings.discord_enabled" type="checkbox" />
                讓她以 Bot 身分,在指定頻道幫你聊天
              </label>
              <p class="hint">
                {{ discordTokenSet ? "✅ Token 已設定(存於 Windows 認證管理員)" : "尚未設定 Bot Token。" }}
              </p>
              <input
                v-model="discordToken"
                type="password"
                :placeholder="discordTokenSet ? '貼上新 Token 可覆蓋,留空維持不變' : '貼上 Discord Bot Token'"
              />
              <label>
                指定頻道 ID(每行一個,只在這些頻道回話)
                <textarea
                  v-model="discordChannelsText"
                  rows="3"
                  placeholder="1503277429748269067&#10;1501094539769811088"
                ></textarea>
              </label>
              <button class="action" :disabled="!settings.discord_enabled" @click="onDiscordConnect">
                🔗 儲存並連線
              </button>
              <p v-if="discordStatus" class="hint">{{ discordStatus }}</p>
              <p class="hint warn">
                ⚠️ 必須是 Discord「Bot」(不可用你本人帳號,那叫 self-bot 會被封號)。Bot 後台要開啟
                <code>MESSAGE CONTENT INTENT</code>。改了 Token/頻道後需重啟 App 才生效。
              </p>
            </section>

            <section>
              <h3>Spotify 播放控制</h3>
              <label class="check">
                <input v-model="settings.spotify_enabled" type="checkbox" />
                讓她幫你控制 Spotify(說「放周杰倫」「暫停」「下一首」「大聲點」)
              </label>
              <p class="hint">
                {{ spotifyLinked ? "✅ 已連結 Spotify" : "尚未連結。填入 Client ID 後按下方按鈕授權。" }}
              </p>
              <label>
                Spotify Client ID
                <input
                  v-model="settings.spotify_client_id"
                  type="text"
                  placeholder="從 Spotify 開發者後台取得"
                />
              </label>
              <button class="action" :disabled="!settings.spotify_enabled" @click="onSpotifyConnect">
                🎵 儲存並連結 Spotify
              </button>
              <p v-if="spotifyStatus" class="hint">{{ spotifyStatus }}</p>
              <p class="hint warn">
                ⚠️ 控制播放需要 <b>Spotify Premium</b>,且要有一台正在執行的 Spotify(手機或電腦)當作播放裝置。
                在開發者後台新增 App 時,Redirect URI 請填 <code>http://127.0.0.1:8888/callback</code>。
              </p>
            </section>

            <section>
              <h3>區網遠端聊天</h3>
              <label class="check">
                <input v-model="settings.remote_enabled" type="checkbox" />
                開放同網段其他電腦/手機用瀏覽器跟她聊天、下指令
              </label>
              <label>
                綁定 IP(留空 = <code>0.0.0.0</code> 全部網卡;要限定某張網卡就填它的 IPv4)
                <input
                  v-model="settings.remote_host"
                  type="text"
                  :placeholder="remoteLocalIpList[0] ? `留空即可,或填 ${remoteLocalIpList[0]}` : '留空 = 0.0.0.0'"
                />
              </label>
              <label>
                埠
                <input v-model.number="settings.remote_port" type="number" min="1" max="65535" />
              </label>
              <button class="action" :disabled="!settings.remote_enabled" @click="onRemoteConnect">
                🌐 儲存並啟動
              </button>
              <p v-if="remoteStatusText" class="hint">{{ remoteStatusText }}</p>
              <p v-if="remoteUrls.length" class="hint">
                其他電腦用瀏覽器開:
                <span v-for="u in remoteUrls" :key="u"><code>{{ u }}</code>&nbsp;</span>
              </p>
              <p v-else-if="remoteLocalIpList.length" class="hint">
                這台電腦的區網位址大概是 <code>{{ remoteLocalIpList[0] }}</code>;啟動後別台電腦開
                <code>http://{{ remoteLocalIpList[0] }}:{{ settings.remote_port }}/</code>
              </p>
              <p class="hint warn">
                ⚠️ 遠端聊天**需要登入**(下面建立帳號發給要用的人)。對方裝置需與這台在同一個區域網路。
                遠端不開放開網頁、讀剪貼簿、改原始碼。
              </p>

              <h3 style="margin-top:14px">網頁登入帳號</h3>
              <p class="hint">
                遠端網頁要登入才能聊天;帳號密碼存在本機、由你管理。<b>每個帳號各自有獨立的記憶</b>(私密),
                外加大家共用的記憶(shared)。
              </p>
              <div class="mem-actions">
                <input v-model="newWebUser" type="text" placeholder="帳號" />
                <input v-model="newWebPass" type="password" placeholder="密碼" />
                <button class="action" @click="onAddWebUser">新增帳號</button>
              </div>
              <ul v-if="webUsers.length" class="checkpoints">
                <li v-for="u in webUsers" :key="u">
                  <div class="cp-info"><span class="cp-note">👤 {{ u }}</span></div>
                  <button class="cp-restore" @click="onDeleteWebUser(u)">刪除</button>
                </li>
              </ul>
              <p v-if="webUserStatus" class="hint">{{ webUserStatus }}</p>

              <p class="hint" style="margin-top:10px">
                <b>邀請碼</b>(一次性):產生後給人,他們在網頁就能用「帳號 + 密碼 + 邀請碼」自助註冊。
              </p>
              <div class="mem-actions">
                <button class="action" @click="onCreateInvite">🎟️ 產生邀請碼</button>
              </div>
              <ul v-if="invites.length" class="checkpoints">
                <li v-for="iv in invites" :key="iv.code">
                  <div class="cp-info">
                    <span class="cp-note"><code>{{ iv.code }}</code></span>
                    <span class="cp-meta">{{ iv.used_by ? ("已被 " + iv.used_by + " 用掉") : "未使用" }}</span>
                  </div>
                  <button class="cp-restore" @click="onDeleteInvite(iv.code)">刪除</button>
                </li>
              </ul>
              <p v-if="inviteStatus" class="hint">{{ inviteStatus }}</p>
            </section>
          </div>

          <!-- ===== 進階 ===== -->
          <div v-show="activeTab === 'advanced'">
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
              <label v-if="settings.self_dev_enabled" class="check">
                <input v-model="settings.self_dev_auto_verify" type="checkbox" />
                改完自動驗證,沒過自動還原(編譯閘)
              </label>
              <p v-if="settings.self_dev_enabled" class="hint">
                她改完一個檔會自動驗證——前端 <code>.ts/.vue</code> 跑型別檢查、<code>.rs</code> 跑 cargo check、
                <code>.json</code> 驗格式;沒過就<b>自動還原那個檔</b>到修改前(不會動到其他檔)。
                做多檔重構(單檔中間狀態還不能編譯)時可關掉。
              </p>
              <p v-if="settings.self_dev_enabled" class="hint warn">
                ⚠️ 每次改檔都會先跳確認卡片、並自動建立 git 還原點;改壞了叫她「還原修改」或自己
                <code>git reset --hard</code>。需要 Agent 工具開啟、且專案是 git 倉庫。
              </p>
              <template v-if="settings.self_dev_enabled">
                <button class="action" @click="loadCheckpoints">🕘 載入還原點(時間軸)</button>
                <p v-if="checkpointStatus" class="hint">{{ checkpointStatus }}</p>
                <ul v-if="checkpoints.length" class="checkpoints">
                  <li v-for="cp in checkpoints" :key="cp.sha">
                    <div class="cp-info">
                      <span class="cp-note">{{ cp.note }}</span>
                      <span class="cp-meta"><code>{{ cp.sha }}</code> · {{ cp.date }}</span>
                    </div>
                    <button class="cp-restore" @click="onRestoreCheckpoint(cp)">還原</button>
                  </li>
                </ul>
              </template>
            </section>
          </div>
        </template>
        <p v-else class="hint">載入設定中…</p>
      </div>
    </div>

    <footer class="win-footer">
      <span class="status">{{ status }}</span>
      <button class="primary" @click="save">儲存</button>
    </footer>
  </div>
</template>

<style scoped>
.settings-root {
  position: fixed;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: #f4f5f7;
  font-size: 13px;
  color: #333;
}
.win-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 14px;
  font-weight: 600;
  background: #fff;
  border-bottom: 1px solid #e5e7eb;
}
.icon-btn {
  border: none;
  background: none;
  cursor: pointer;
  font-size: 14px;
}
.win-main {
  flex: 1;
  display: flex;
  min-height: 0;
}
/* ---------- 關於 / 更新 ---------- */
.ver-list {
  list-style: none;
  padding: 0;
  margin: 0 0 10px;
}
.ver-list li {
  display: flex;
  justify-content: space-between;
  padding: 5px 0;
  border-bottom: 1px dashed #e5e7eb;
}
.ver-list code {
  font-weight: 600;
}
.dim {
  color: #888;
}
.hint.ok {
  color: #177245;
}
.hint.warn {
  color: #b3261e;
}
.upd-progress {
  margin: 8px 0;
}
.upd-bar {
  height: 8px;
  border-radius: 4px;
  background: #e5e7eb;
  overflow: hidden;
}
.upd-bar-fill {
  height: 100%;
  background: #4f8cff;
  transition: width 0.2s ease;
}
.upd-bytes {
  display: block;
  margin-top: 4px;
  font-size: 12px;
  color: #666;
}
.upd-card {
  margin-top: 10px;
  padding: 10px 12px;
  border: 1px solid #cdd9f0;
  border-radius: 8px;
  background: #f6f9ff;
}
.upd-notes {
  max-height: 180px;
  overflow: auto;
  white-space: pre-wrap;
  font: 12px/1.5 inherit;
  background: #fff;
  border: 1px solid #e5e7eb;
  border-radius: 6px;
  padding: 8px;
  margin: 8px 0;
}
.upd-actions {
  display: flex;
  gap: 8px;
}
.upd-actions .primary {
  flex: 1;
}
.backup-list {
  margin-top: 8px;
}
.backup-list li {
  font-size: 12px;
}
.tabs {
  width: 124px;
  flex: none;
  background: #fff;
  border-right: 1px solid #e5e7eb;
  padding: 8px 6px;
  overflow-y: auto;
}
.tab-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  border: none;
  background: none;
  padding: 8px 10px;
  margin-bottom: 2px;
  border-radius: 8px;
  cursor: pointer;
  font-size: 13px;
  color: #555;
  text-align: left;
}
.tab-btn:hover {
  background: #f1f3f5;
}
.tab-btn.active {
  background: #e8f0fe;
  color: #3b6fd4;
  font-weight: 600;
}
.tab-ico {
  font-size: 15px;
}
.tab-content {
  flex: 1;
  overflow-y: auto;
  padding: 14px 16px;
  min-width: 0;
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
.size-row {
  display: flex;
  gap: 8px;
}
.size-row label {
  flex: 1;
}
.fish-divider {
  border: none;
  border-top: 1px solid #e5e7eb;
  margin: 10px 0;
}
/* 泡泡樣式預覽:中性背景,深/淺字泡泡都看得清楚 */
.bubble-preview {
  position: relative;
  height: 92px;
  margin: 8px 0;
  border-radius: 10px;
  background: linear-gradient(135deg, #7a8aa8, #aab4c6);
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
.checkpoints {
  list-style: none;
  margin: 6px 0 0;
  padding: 0;
  max-height: 220px;
  overflow-y: auto;
  border: 1px solid #eee;
  border-radius: 8px;
}
.checkpoints li {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border-bottom: 1px solid #f3f3f3;
}
.checkpoints li:last-child {
  border-bottom: none;
}
.cp-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.cp-note {
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cp-meta {
  font-size: 11px;
  color: #999;
}
.cp-meta code {
  background: #f0f0f0;
  padding: 0 3px;
  border-radius: 3px;
}
.cp-restore {
  flex: none;
  border: 1px solid #ddd;
  background: #fff;
  border-radius: 12px;
  padding: 3px 12px;
  font-size: 12px;
  cursor: pointer;
}
.cp-restore:hover {
  background: #f7f7f7;
}
.persona-summary {
  margin: 0 0 8px;
  padding: 8px 10px;
  background: #fff5f8;
  border: 1px solid #ffd5e2;
  border-radius: 8px;
  font-size: 13px;
  line-height: 1.5;
  color: #555;
  white-space: pre-wrap;
}
.win-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 14px;
  background: #fff;
  border-top: 1px solid #e5e7eb;
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
.action {
  border: none;
  background: #5865f2;
  color: #fff;
  padding: 6px 14px;
  border-radius: 14px;
  cursor: pointer;
  font-size: 12px;
  margin-top: 4px;
}
.action:hover {
  background: #4752c4;
}
.action:disabled {
  background: #c7c9d9;
  cursor: not-allowed;
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
.fish-settings {
  margin: 8px 0;
  padding: 8px 12px;
  background: #f8f9ff;
  border-radius: 8px;
  border: 1px solid #e0e4f0;
}
.fish-settings summary {
  cursor: pointer;
  font-weight: 600;
  color: #5b6abf;
  font-size: 13px;
  margin-bottom: 6px;
}
.fish-settings label {
  display: block;
  margin: 6px 0;
  font-size: 12px;
  color: #555;
}
.fish-settings input,
.fish-settings textarea {
  width: 100%;
  box-sizing: border-box;
  margin-top: 2px;
  padding: 5px 8px;
  border: 1px solid #d0d5e0;
  border-radius: 6px;
  font-size: 13px;
  font-family: inherit;
}
.fish-settings textarea {
  resize: vertical;
  min-height: 40px;
}
</style>
