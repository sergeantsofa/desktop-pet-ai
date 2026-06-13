/**
 * TTS 語音輸出(M2):
 * - Edge 神經網路語音(甜美自然、免金鑰、需網路)→ MP3 餵 model.speak() 真實對嘴
 * - Piper sidecar(全本地)→ WAV 餵 model.speak()
 * - 退回 WebView2 內建 Web Speech API(speechSynthesis)+ 正弦口型近似
 * 引擎選擇:"auto"(Edge → Piper → 系統,逐級退回)| "edge" | "piper" | "system"。
 * 設定存 localStorage(純前端功能,不進 Rust settings.json)。
 */
import { setTalking, speakWithLipsync, stopLipsync } from "../live2d/stage";
import { isTauri } from "../llm/api";
import { speechStatus, synthesize, synthesizeEdge } from "./native";

export type TtsEngine = "auto" | "edge" | "piper" | "system";

export interface TtsSettings {
  enabled: boolean;
  engine: TtsEngine;
  /** Edge 語音短名,如 zh-CN-XiaoyiNeural */
  edgeVoice: string;
  /** SpeechSynthesisVoice.voiceURI;空字串 = 自動挑中文語音(僅系統語音引擎) */
  voice: string;
  /** 語速 0.5 ~ 2(Piper 以 length_scale = 1/rate 換算) */
  rate: number;
  /** 音量 0 ~ 1 */
  volume: number;
}

/** Edge 語音預設清單(顯示名稱 → 短名) */
export const EDGE_VOICES: Array<{ id: string; label: string }> = [
  { id: "zh-CN-XiaoyiNeural", label: "曉伊(甜美少女)" },
  { id: "zh-CN-XiaoxiaoNeural", label: "曉曉(活潑自然)" },
  { id: "zh-CN-XiaoshuangNeural", label: "曉雙(軟萌童音)" },
  { id: "zh-TW-HsiaoChenNeural", label: "曉臻(台灣腔・溫柔)" },
  { id: "zh-TW-HsiaoYuNeural", label: "曉雨(台灣腔)" },
  { id: "zh-CN-YunxiNeural", label: "雲希(陽光少年)" },
];

const STORAGE_KEY = "tts-settings";

const DEFAULTS: TtsSettings = {
  enabled: true,
  engine: "auto",
  edgeVoice: "zh-CN-XiaoyiNeural",
  voice: "",
  rate: 1,
  volume: 1,
};

export function loadTtsSettings(): TtsSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    return { ...DEFAULTS, ...(JSON.parse(raw) as Partial<TtsSettings>) };
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveTtsSettings(settings: TtsSettings): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
}

export function ttsSupported(): boolean {
  return typeof window !== "undefined" && "speechSynthesis" in window;
}

/** 取得系統語音列表。voiceschanged 是非同步的,首次呼叫可能要等一下。 */
export function listVoices(): Promise<SpeechSynthesisVoice[]> {
  if (!ttsSupported()) return Promise.resolve([]);
  const now = window.speechSynthesis.getVoices();
  if (now.length > 0) return Promise.resolve(now);
  return new Promise((resolve) => {
    const timer = window.setTimeout(() => resolve(window.speechSynthesis.getVoices()), 1500);
    window.speechSynthesis.addEventListener(
      "voiceschanged",
      () => {
        window.clearTimeout(timer);
        resolve(window.speechSynthesis.getVoices());
      },
      { once: true }
    );
  });
}

function pickVoice(voices: SpeechSynthesisVoice[], preferred: string): SpeechSynthesisVoice | null {
  if (preferred) {
    const exact = voices.find((v) => v.voiceURI === preferred);
    if (exact) return exact;
  }
  // 自動:優先繁中,其次任何中文
  return (
    voices.find((v) => /^zh(-|_)?(TW|HK|Hant)/i.test(v.lang)) ??
    voices.find((v) => /^zh/i.test(v.lang)) ??
    null
  );
}

/** 朗讀前清掉不適合唸出來的內容:括號內舞台動作、顏文字、網址。 */
function cleanForSpeech(text: string): string {
  return text
    .replace(/[((][^))]*[))]/g, " ")
    .replace(/https?:\/\/\S+/g, " ")
    .replace(/[~~…]+/g, "。")
    .replace(/\s+/g, " ")
    .trim();
}

/* ---------------- 播放 ---------------- */

let plainAudio: HTMLAudioElement | null = null;

/**
 * 朗讀一段文字(會先停掉前一段)。口型由本模組驅動,呼叫端不用管。
 * `override`:用指定設定取代已儲存設定(設定面板「試聽」用)。
 */
export async function speak(text: string, override?: TtsSettings): Promise<void> {
  const settings = override ?? loadTtsSettings();
  const speakable = cleanForSpeech(text);
  if (!settings.enabled || !speakable) return;

  stopSpeaking();

  const engine = settings.engine;
  if (isTauri) {
    if ((engine === "auto" || engine === "edge") && (await speakWithEdge(speakable, settings))) {
      return;
    }
    if ((engine === "auto" || engine === "piper") && (await speakWithPiper(speakable, settings))) {
      return;
    }
  }
  speakWithSystemVoice(speakable, settings);
}

/** Edge 路徑:雲端神經網路語音 → MP3 blob → model.speak 真實對嘴。 */
async function speakWithEdge(text: string, settings: TtsSettings): Promise<boolean> {
  try {
    const mp3 = await synthesizeEdge(text, settings.edgeVoice, settings.rate);
    return playBuffer(mp3, "audio/mpeg", settings.volume);
  } catch {
    return false; // 斷網/被擋 → 退回本地引擎
  }
}

/** Piper 路徑:本地合成 WAV → blob URL → model.speak 真實對嘴。 */
async function speakWithPiper(text: string, settings: TtsSettings): Promise<boolean> {
  try {
    const status = await speechStatus();
    if (!status.piper) return false;
    const wav = await synthesize(text, 1 / settings.rate);
    return playBuffer(wav, "audio/wav", settings.volume);
  } catch {
    return false; // 合成失敗 → 退回系統語音
  }
}

/** 播放音訊 buffer:優先 model.speak(對嘴),模型未載入時用 Audio 直接播。 */
async function playBuffer(buf: ArrayBuffer, mime: string, volume: number): Promise<boolean> {
  const url = URL.createObjectURL(new Blob([buf], { type: mime }));
  const release = () => URL.revokeObjectURL(url);
  if (speakWithLipsync(url, { volume, onFinish: release, onError: release })) {
    return true;
  }
  plainAudio = new Audio(url);
  plainAudio.volume = volume;
  plainAudio.onended = release;
  plainAudio.onerror = release;
  await plainAudio.play();
  return true;
}

/** 系統語音路徑:speechSynthesis + 正弦口型近似 */
function speakWithSystemVoice(text: string, settings: TtsSettings): void {
  if (!ttsSupported()) return;
  const utter = new SpeechSynthesisUtterance(text);
  void listVoices().then((voices) => {
    const voice = pickVoice(voices, settings.voice);
    if (voice) {
      utter.voice = voice;
      utter.lang = voice.lang;
    } else {
      utter.lang = "zh-TW";
    }
    utter.rate = settings.rate;
    utter.volume = settings.volume;
    utter.onstart = () => setTalking(true);
    utter.onend = () => setTalking(false);
    utter.onerror = () => setTalking(false);
    window.speechSynthesis.speak(utter);
  });
}

/** 停止所有引擎的朗讀與口型 */
export function stopSpeaking(): void {
  if (ttsSupported()) window.speechSynthesis.cancel();
  setTalking(false);
  stopLipsync();
  if (plainAudio) {
    plainAudio.pause();
    plainAudio = null;
  }
}
