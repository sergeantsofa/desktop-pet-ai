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
  /** M3:模型正在使用某個工具(label 為中文名,如「查時間」) */
  onTool?: (label: string) => void;
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

export async function healthCheck(): Promise<Record<string, boolean>> {
  return invoke("health_check");
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
    active.get(e.payload.requestId)?.onTool?.(e.payload.label);
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
