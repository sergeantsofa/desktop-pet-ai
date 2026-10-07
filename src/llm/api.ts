/**
 * Rust LLM 核心的前端封裝:設定、金鑰、健康檢查、串流對話。
 * 非 Tauri 環境(純 vite dev)會走 mock,方便 UI 開發。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const isTauri = "__TAURI_INTERNALS__" in window;

export interface ProviderCfg {
  id: string;
  name: string;
  base_url: string;
  uses_key: boolean;
}

export interface TaskRoute {
  provider: string;
  model: string;
}

export interface Persona {
  name: string;
  system_prompt: string;
}

export interface Settings {
  providers: ProviderCfg[];
  routing: Record<string, TaskRoute>;
  fallback: TaskRoute;
  fallback_to_local: boolean;
  persona: Persona;
  context_turns: number;
  agent_enabled: boolean;
  watch_screenshots: boolean;
  screenshot_dir: string;
  self_dev_enabled: boolean;
  self_dev_root: string;
  self_dev_auto_verify: boolean;
  discord_enabled: boolean;
  discord_channels: string[];
  spotify_enabled: boolean;
  spotify_client_id: string;
  remote_enabled: boolean;
  remote_host: string;
  remote_port: number;
  embed_model: string;
  fish_autostart: boolean;
  fish_launch_cmd: string;
  fish_cwd: string;
  fish_api_base: string;
  game_kb_enabled: boolean;
}

/* ---------------- 遊戲知識庫 ---------------- */

export interface GameKnowledge {
  id: number;
  content: string;
}

/** 教她一條遊戲知識;回傳新的 id */
export async function gamekbAdd(text: string): Promise<number> {
  return invoke<number>("gamekb_add", { text });
}

/** 列出所有遊戲知識 */
export async function gamekbList(): Promise<GameKnowledge[]> {
  if (!isTauri) return [];
  try {
    return await invoke<GameKnowledge[]>("gamekb_list");
  } catch {
    return [];
  }
}

/** 刪一條遊戲知識;回傳刪除筆數 */
export async function gamekbDelete(id: number): Promise<number> {
  return invoke<number>("gamekb_delete", { id });
}

/** 啟動本地 Fish Speech API server(用設定裡的指令);回傳人類可讀狀態 */
export async function fishStart(): Promise<string> {
  if (!isTauri) return "(開發模式)略過";
  return invoke<string>("fish_start");
}

/** 停止由桌寵啟動的 Fish */
export async function fishStop(): Promise<string> {
  if (!isTauri) return "(開發模式)略過";
  return invoke<string>("fish_stop");
}

export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
}

export interface StreamHandlers {
  onDelta: (delta: string) => void;
  onDone: (full: string) => void;
  onError: (message: string) => void;
  onFallback?: (from: string, to: string) => void;
  /** M3:模型正在使用某個工具(label 為中文名;name 為工具代號,如 dev_write_file) */
  onTool?: (label: string, name: string) => void;
}

export async function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

export async function saveSettings(settings: Settings): Promise<void> {
  return invoke("save_settings", { settings });
}

export async function setApiKey(providerId: string, key: string): Promise<void> {
  return invoke("set_api_key", { providerId, key });
}

export async function hasApiKey(providerId: string): Promise<boolean> {
  return invoke("has_api_key", { providerId });
}

/** 依目前設定嘗試連線 Discord;回傳人類可讀狀態字串 */
export async function discordConnect(): Promise<string> {
  if (!isTauri) return "(開發模式)略過 Discord 連線";
  return invoke("discord_connect");
}

export interface DiscordActivity {
  author: string;
  text: string;
  reply: string;
}

/** 訂閱「她在 Discord 有回話」事件,讓桌面端能冒泡泡反應 */
export async function onDiscordActivity(cb: (info: DiscordActivity) => void): Promise<void> {
  if (!isTauri) return;
  await listen<DiscordActivity>("discord-activity", (e) => cb(e.payload));
}

/** 開瀏覽器授權並連結 Spotify;回傳人類可讀狀態字串 */
export async function spotifyConnect(): Promise<string> {
  if (!isTauri) return "(開發模式)略過 Spotify 連結";
  return invoke("spotify_connect");
}

/** 輪詢目前播放狀態(+ 可能的 BPM),供「跟著音樂搖擺」用 */
export async function spotifyPlaybackState(): Promise<{ playing: boolean; bpm: number | null }> {
  if (!isTauri) return { playing: false, bpm: null };
  try {
    return await invoke("spotify_playback_state");
  } catch {
    return { playing: false, bpm: null };
  }
}

/* ---------------- 區網遠端聊天 ---------------- */

export interface RemoteStatus {
  running: boolean;
  bound: string;
  urls: string[];
}

/** 依目前設定啟動/重啟遠端聊天伺服器;回傳人類可讀狀態字串 */
export async function remoteConnect(): Promise<string> {
  if (!isTauri) return "(開發模式)略過遠端啟動";
  return invoke("remote_connect");
}

/** 查目前遠端伺服器狀態(供 UI 顯示連結) */
export async function remoteStatus(): Promise<RemoteStatus> {
  if (!isTauri) return { running: false, bound: "", urls: [] };
  try {
    return await invoke<RemoteStatus>("remote_status");
  } catch {
    return { running: false, bound: "", urls: [] };
  }
}

/** 偵測本機區網 IPv4,供 UI 提示要填哪個位址 */
export async function remoteLocalIps(): Promise<string[]> {
  if (!isTauri) return [];
  try {
    return await invoke<string[]>("remote_local_ips");
  } catch {
    return [];
  }
}

export interface RemoteActivity {
  room: string;
  text: string;
  reply: string;
}

/** 訂閱「有人在遠端網頁找她」事件,讓桌面端冒泡泡反應 */
export async function onRemoteActivity(cb: (info: RemoteActivity) => void): Promise<void> {
  if (!isTauri) return;
  await listen<RemoteActivity>("remote-activity", (e) => cb(e.payload));
}

/* ---------------- 區網遠端:網頁帳號(Phase 2 認證) ---------------- */

/** 建立一個網頁登入帳號(owner 管;帳號密碼存本機 DB) */
export async function authCreateUser(username: string, password: string): Promise<void> {
  return invoke("auth_create_user", { username, password });
}

/** 列出所有網頁帳號 */
export async function authListUsers(): Promise<string[]> {
  if (!isTauri) return [];
  try {
    return await invoke<string[]>("auth_list_users");
  } catch {
    return [];
  }
}

/** 刪除網頁帳號(連同該帳號的私密記憶一起清掉);回傳刪除筆數 */
export async function authDeleteUser(username: string): Promise<number> {
  return invoke<number>("auth_delete_user", { username });
}

export interface Invite {
  code: string;
  used_by: string | null;
}

/** 產生一組一次性邀請碼;回傳該碼 */
export async function authCreateInvite(): Promise<string> {
  return invoke<string>("auth_create_invite");
}

/** 列出所有邀請碼(含使用狀態) */
export async function authListInvites(): Promise<Invite[]> {
  if (!isTauri) return [];
  try {
    return await invoke<Invite[]>("auth_list_invites");
  } catch {
    return [];
  }
}

/** 刪除邀請碼;回傳刪除筆數 */
export async function authDeleteInvite(code: string): Promise<number> {
  return invoke<number>("auth_delete_invite", { code });
}

export async function healthCheck(): Promise<Record<string, boolean>> {
  return invoke("health_check");
}

/** 列出某 provider 可用的模型(Ollama 回已安裝清單);失敗回空陣列 */
export async function listModels(providerId: string): Promise<string[]> {
  if (!isTauri) return [];
  try {
    return await invoke<string[]>("list_models", { providerId });
  } catch {
    return [];
  }
}

/* ---------------- 串流對話 ---------------- */

const active = new Map<string, StreamHandlers>();
let listenersReady = false;

async function ensureListeners(): Promise<void> {
  if (listenersReady) return;
  listenersReady = true;
  await listen<{ requestId: string; delta: string }>("chat-delta", (e) => {
    active.get(e.payload.requestId)?.onDelta(e.payload.delta);
  });
  await listen<{ requestId: string; content: string }>("chat-done", (e) => {
    active.get(e.payload.requestId)?.onDone(e.payload.content);
    active.delete(e.payload.requestId);
  });
  await listen<{ requestId: string; message: string }>("chat-error", (e) => {
    active.get(e.payload.requestId)?.onError(e.payload.message);
    active.delete(e.payload.requestId);
  });
  await listen<{ requestId: string; from: string; to: string }>("chat-fallback", (e) => {
    active.get(e.payload.requestId)?.onFallback?.(e.payload.from, e.payload.to);
  });
  await listen<{ requestId: string; name: string; label: string }>("chat-tool", (e) => {
    active.get(e.payload.requestId)?.onTool?.(e.payload.label, e.payload.name);
  });
}

/**
 * 送出對話,回傳 requestId。結果經 handlers 串流回來。
 * persist=false 時這次互動不落對話紀錄(主動關心/提醒的合成指令用)。
 */
export async function chatStream(
  task: "chat" | "coder" | "reasoner",
  messages: ChatMessage[],
  handlers: StreamHandlers,
  persist = true
): Promise<string> {
  const requestId = crypto.randomUUID();

  if (!isTauri) {
    // 純瀏覽器開發 mock
    window.setTimeout(() => {
      const text = "[happy]我是開發模式的假回應喔~(非 Tauri 環境)";
      handlers.onDelta(text);
      handlers.onDone(text);
    }, 400);
    return requestId;
  }

  await ensureListeners();
  active.set(requestId, handlers);
  invoke("chat_stream", { requestId, task, messages, persist }).catch((err) => {
    if (active.has(requestId)) {
      handlers.onError(String(err));
      active.delete(requestId);
    }
  });
  return requestId;
}

/**
 * 看截圖(M5):Rust 截取主螢幕 → 視覺模型描述。結果走同一組 chat-* 事件。
 * prompt 留空時 Rust 會用預設「看看螢幕並評論」。
 */
export async function visionChat(prompt: string, handlers: StreamHandlers): Promise<string> {
  const requestId = crypto.randomUUID();
  if (!isTauri) {
    window.setTimeout(() => {
      const text = "[happy](開發模式)我看到一個很棒的螢幕喔~";
      handlers.onDelta(text);
      handlers.onDone(text);
    }, 400);
    return requestId;
  }
  await ensureListeners();
  active.set(requestId, handlers);
  invoke("vision_chat", { requestId, prompt }).catch((err) => {
    if (active.has(requestId)) {
      handlers.onError(String(err));
      active.delete(requestId);
    }
  });
  return requestId;
}

/** 看指定圖片檔(M5.5:截圖資料夾監看到新圖時)。結果走同一組 chat-* 事件。 */
export async function visionChatFile(
  path: string,
  prompt: string,
  handlers: StreamHandlers
): Promise<string> {
  const requestId = crypto.randomUUID();
  if (!isTauri) return requestId;
  await ensureListeners();
  active.set(requestId, handlers);
  invoke("vision_chat_file", { requestId, path, prompt }).catch((err) => {
    if (active.has(requestId)) {
      handlers.onError(String(err));
      active.delete(requestId);
    }
  });
  return requestId;
}

/** 放棄追蹤某個請求(UI 已不關心時) */
export function abandonRequest(requestId: string): void {
  active.delete(requestId);
}

/** 要求 Rust 端中止串流並停止追蹤(送新訊息打斷舊回應時用) */
export function cancelChat(requestId: string): void {
  active.delete(requestId);
  if (isTauri) void invoke("cancel_chat", { requestId }).catch(() => undefined);
}

/* ---------------- 記憶(M4) ---------------- */

/** 載入最近的對話紀錄(重啟後接續上下文) */
export async function loadRecentHistory(): Promise<ChatMessage[]> {
  if (!isTauri) return [];
  try {
    return await invoke<ChatMessage[]>("load_recent_history");
  } catch {
    return [];
  }
}

/** 清空長期記憶;回傳刪除筆數 */
export async function clearMemories(): Promise<number> {
  return invoke<number>("clear_memories");
}

/** 清空對話紀錄;回傳刪除筆數 */
export async function clearHistory(): Promise<number> {
  return invoke<number>("clear_history");
}

/** 把舊記憶補算語意向量(讓它們也能被語意檢索到);回傳補了幾條 */
export async function backfillEmbeddings(): Promise<number> {
  if (!isTauri) return 0;
  return invoke<number>("backfill_embeddings");
}

/** 取目前的人格摘要(她眼中的你);沒有回 null */
export async function getPersonaSummary(): Promise<string | null> {
  if (!isTauri) return null;
  try {
    return (await invoke<string | null>("get_persona_summary")) ?? null;
  } catch {
    return null;
  }
}

/** 強制重算人格摘要;回傳新摘要 */
export async function refreshPersonaSummary(): Promise<string> {
  return invoke<string>("refresh_persona_summary");
}

/* ---------------- Agent 權限(M3) ---------------- */

export interface PermissionRequest {
  requestId: string;
  callId: string;
  tool: string;
  label: string;
  detail: string;
}

/** 回應工具權限請求(允許/拒絕) */
export function respondPermission(callId: string, allow: boolean): void {
  if (isTauri) {
    void invoke("agent_permission_response", { callId, allow }).catch(() => undefined);
  }
}

/* ---------------- 自我修改:還原點時間軸(P1) ---------------- */

export interface Checkpoint {
  sha: string;
  date: string;
  note: string;
}

/** 列出最近的還原點(git commit;🐾 開頭的是她的自我修改快照) */
export async function devListCheckpoints(): Promise<Checkpoint[]> {
  if (!isTauri) return [];
  return invoke<Checkpoint[]>("dev_list_checkpoints");
}

/** 還原到指定還原點(會先把現狀自動快照,可再還原回來);回傳人類可讀狀態 */
export async function devRestoreCheckpoint(sha: string): Promise<string> {
  return invoke<string>("dev_restore_checkpoint", { sha });
}
