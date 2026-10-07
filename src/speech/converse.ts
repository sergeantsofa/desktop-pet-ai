/**
 * 連續對話模式(免持):麥克風持續開著聽,用 WebAudio 即時算音量做語音活動偵測(VAD)。
 *  - 你一開口 → onSpeechStart(讓桌寵立刻閉嘴、停掉正在生成的回答,等你說完)。
 *  - 你講完、靜音超過 silenceMs → 自動把這段話切出來轉成 16k WAV → onUtterance(去辨識+回答)。
 *  - 再呼叫 stopConverse() 才停止(由 App 的 Ctrl+Shift+D 切換)。
 * 回授/桌寵自己的聲音被麥克風收到 → 靠 getUserMedia 的 echoCancellation 消除。
 */
import { pcmToWav16k } from "./recorder";

export interface ConverseOptions {
  /** 講完後靜音多久(毫秒)算斷句 */
  silenceMs: number;
  /** 使用者開口(每段話開頭觸發一次)→ 打斷桌寵 */
  onSpeechStart: () => void;
  /** 一段話結束 → 16kHz mono WAV */
  onUtterance: (wav: Uint8Array) => void;
}

const PROC_SIZE = 4096;
/** 太短的聲音(< 這個毫秒數的有聲內容)當噪音丟掉,不送辨識 */
const MIN_UTTERANCE_MS = 350;

let active = false;
let ctx: AudioContext | null = null;
let stream: MediaStream | null = null;
let source: MediaStreamAudioSourceNode | null = null;
let processor: ScriptProcessorNode | null = null;
let opts: ConverseOptions | null = null;

let sampleRate = 48_000;
let buffers: Float32Array[] = [];
let voicedMs = 0;
let utteranceActive = false;
let lastVoiceTs = 0;
let noiseFloor = 0.01;

export function isConversing(): boolean {
  return active;
}

/** 開始連續對話。失敗(無麥克風/拒絕權限)時 throw。 */
export async function startConverse(o: ConverseOptions): Promise<void> {
  if (active) return;
  opts = o;
  stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
  });
  ctx = new AudioContext();
  sampleRate = ctx.sampleRate;
  source = ctx.createMediaStreamSource(stream);
  processor = ctx.createScriptProcessor(PROC_SIZE, 1, 1);
  resetUtterance();
  noiseFloor = 0.01;
  processor.onaudioprocess = onAudio;
  source.connect(processor);
  // 必須連到 destination 才會持續觸發 onaudioprocess;我們不寫 output buffer,所以不會把麥克風聲音播出去(無回授)。
  processor.connect(ctx.destination);
  active = true;
}

/** 停止連續對話並釋放麥克風。 */
export function stopConverse(): void {
  active = false;
  if (processor) {
    processor.onaudioprocess = null;
    processor.disconnect();
  }
  source?.disconnect();
  stream?.getTracks().forEach((t) => t.stop());
  void ctx?.close();
  processor = null;
  source = null;
  stream = null;
  ctx = null;
  opts = null;
  resetUtterance();
}

function resetUtterance(): void {
  buffers = [];
  voicedMs = 0;
  utteranceActive = false;
  lastVoiceTs = 0;
}

function onAudio(e: AudioProcessingEvent): void {
  if (!active || !opts) return;
  const input = e.inputBuffer.getChannelData(0);

  // 這一幀的 RMS 音量
  let sum = 0;
  for (let i = 0; i < input.length; i++) sum += input[i] * input[i];
  const rms = Math.sqrt(sum / input.length);
  const frameMs = (input.length / sampleRate) * 1000;
  const now = performance.now();

  // 動態門檻:跟著環境噪音底走,安靜環境也不會誤判
  const gate = noiseFloor * 3 + 0.012;
  const isVoice = rms > gate;

  if (isVoice) {
    if (!utteranceActive) {
      utteranceActive = true;
      opts.onSpeechStart(); // 開口 → 打斷桌寵
    }
    buffers.push(new Float32Array(input)); // 必須複製,inputBuffer 下一幀會被重用
    voicedMs += frameMs;
    lastVoiceTs = now;
  } else {
    // 只在沒講話時更新噪音底
    noiseFloor = 0.95 * noiseFloor + 0.05 * rms;
    if (utteranceActive) {
      buffers.push(new Float32Array(input)); // 收尾的靜音也留一點,聽起來自然
      if (now - lastVoiceTs >= opts.silenceMs) {
        finalize();
      }
    }
  }
}

function finalize(): void {
  const o = opts;
  const segs = buffers;
  const voiced = voicedMs;
  resetUtterance(); // 立刻歸零,繼續聽下一句
  if (!o || voiced < MIN_UTTERANCE_MS) return; // 太短 → 當噪音忽略

  let total = 0;
  for (const s of segs) total += s.length;
  const merged = new Float32Array(total);
  let off = 0;
  for (const s of segs) {
    merged.set(s, off);
    off += s.length;
  }
  void pcmToWav16k(merged, sampleRate).then((wav) => o.onUtterance(wav));
}
