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

/** Whisper 辨識 16kHz mono PCM16 WAV。 */
export async function transcribe(wav: Uint8Array): Promise<string> {
  return invoke<string>("stt_transcribe", { wavB64: toBase64(wav) });
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}
