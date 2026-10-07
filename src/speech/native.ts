/**
 * Rust speech 命令封裝(M2):sidecar 狀態、Piper 合成、Whisper 辨識。
 */
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../llm/api";

export interface SpeechStatus {
  dir: string;
  piper: boolean;
  piperVoice: string | null;
  whisper: boolean;
  whisperModel: string | null;
}

const NOT_INSTALLED: SpeechStatus = {
  dir: "",
  piper: false,
  piperVoice: null,
  whisper: false,
  whisperModel: null,
};

export async function speechStatus(): Promise<SpeechStatus> {
  if (!isTauri) return NOT_INSTALLED;
  try {
    return await invoke<SpeechStatus>("speech_status");
  } catch {
    return NOT_INSTALLED;
  }
}

/** Piper 合成,回傳 WAV bytes。失敗時 throw(呼叫端退回系統語音)。 */
export async function synthesize(text: string, lengthScale: number): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("tts_synthesize", { text, lengthScale });
}

/** Edge 神經網路語音合成,回傳 MP3 bytes。需網路;失敗時 throw(呼叫端退回本地引擎)。 */
export async function synthesizeEdge(
  text: string,
  voice: string,
  rate: number
): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("tts_edge", { text, voice, rate });
}

/** Fish Speech 合成,回傳 WAV bytes。經 HTTP API 呼叫本地 Fish Speech 服務。 */
export async function synthesizeFish(
  text: string,
  apiUrl: string,
  refAudio?: string,
  refText?: string,
): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("tts_fish", {
    text,
    apiUrl,
    refAudio: refAudio || null,
    refText: refText || null,
  });
}

/** Whisper 辨識 16kHz mono PCM16 WAV。 */
export async function transcribe(wav: Uint8Array): Promise<string> {
  return invoke<string>("stt_transcribe", { wavB64: toBase64(wav) });
}

/* ---------------- 離線語音元件一鍵安裝 ---------------- */

export interface SpeechSetupProgress {
  done: number;
  total: number;
  label: string;
}

/**
 * 一鍵安裝離線語音元件(Whisper 語音輸入 + Piper 朗讀)。
 *
 * 為什麼需要:安裝版使用者拿不到 `scripts\setup-speech.ps1`,遇到
 * 「找不到 whisper-cli.exe」只能卡住。這裡讓 App 自己下載。
 * 進度走 `speech-setup-progress` 事件。
 *
 * @returns 人類可讀的安裝結果摘要(逐項列出裝了什麼/略過什麼)
 */
export async function setupSpeech(
  opts: { whisper?: boolean; piper?: boolean; whisperModel?: string } = {},
  onProgress?: (p: SpeechSetupProgress) => void
): Promise<string> {
  if (!isTauri) return "(開發模式)略過語音元件安裝";
  const { listen } = await import("@tauri-apps/api/event");
  const un = onProgress
    ? await listen<SpeechSetupProgress>("speech-setup-progress", (e) => onProgress(e.payload))
    : null;
  try {
    return await invoke<string>("setup_speech", {
      whisper: opts.whisper ?? true,
      piper: opts.piper ?? true,
      whisperModel: opts.whisperModel ?? "base",
    });
  } finally {
    un?.();
  }
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}
