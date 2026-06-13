/**
 * 麥克風錄音(M2 語音輸入):MediaRecorder 錄 webm/opus,
 * 結束時用 WebAudio 解碼並重採樣成 Whisper 要的 16kHz 單聲道 PCM16 WAV。
 */

const TARGET_RATE = 16_000;
/** 防呆:最長錄 60 秒,自動結束 */
const MAX_MS = 60_000;

let stream: MediaStream | null = null;
let recorder: MediaRecorder | null = null;
let chunks: Blob[] = [];
let maxTimer: number | undefined;
let autoStopped: ((wav: Uint8Array) => void) | null = null;

export function isRecording(): boolean {
  return recorder?.state === "recording";
}

/**
 * 開始錄音。失敗(無麥克風/拒絕權限)時 throw。
 * onAutoStop:錄滿上限自動結束時回傳結果(手動 stopRecording 不會觸發)。
 */
export async function startRecording(onAutoStop?: (wav: Uint8Array) => void): Promise<void> {
  if (isRecording()) return;
  stream = await navigator.mediaDevices.getUserMedia({
    audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
  });
  chunks = [];
  recorder = new MediaRecorder(stream);
  recorder.ondataavailable = (e) => {
    if (e.data.size > 0) chunks.push(e.data);
  };
  recorder.start();
  autoStopped = onAutoStop ?? null;
  maxTimer = window.setTimeout(() => {
    void stopRecording().then((wav) => autoStopped?.(wav));
  }, MAX_MS);
}

/** 結束錄音並回傳 16kHz mono PCM16 WAV。 */
export async function stopRecording(): Promise<Uint8Array> {
  const rec = recorder;
  if (!rec || rec.state !== "recording") throw new Error("沒有進行中的錄音");
  window.clearTimeout(maxTimer);
  autoStopped = null;

  const blob = await new Promise<Blob>((resolve) => {
    rec.onstop = () => resolve(new Blob(chunks, { type: rec.mimeType }));
    rec.stop();
  });
  cleanup();
  return encodeWav(await resampleToMono16k(blob));
}

/** 放棄錄音(不產出結果) */
export function cancelRecording(): void {
  window.clearTimeout(maxTimer);
  autoStopped = null;
  if (recorder && recorder.state === "recording") recorder.stop();
  cleanup();
}

function cleanup(): void {
  stream?.getTracks().forEach((t) => t.stop());
  stream = null;
  recorder = null;
  chunks = [];
}

async function resampleToMono16k(blob: Blob): Promise<Float32Array> {
  const ctx = new AudioContext();
  let decoded: AudioBuffer;
  try {
    decoded = await ctx.decodeAudioData(await blob.arrayBuffer());
  } finally {
    void ctx.close();
  }
  const frames = Math.max(1, Math.ceil(decoded.duration * TARGET_RATE));
  const off = new OfflineAudioContext(1, frames, TARGET_RATE);
  const src = off.createBufferSource();
  src.buffer = decoded; // 多聲道會自動 down-mix 成單聲道
  src.connect(off.destination);
  src.start();
  const rendered = await off.startRendering();
  return rendered.getChannelData(0);
}

function encodeWav(samples: Float32Array): Uint8Array {
  const dataLen = samples.length * 2;
  const buf = new ArrayBuffer(44 + dataLen);
  const view = new DataView(buf);
  const writeStr = (offset: number, s: string) => {
    for (let i = 0; i < s.length; i++) view.setUint8(offset + i, s.charCodeAt(i));
  };
  writeStr(0, "RIFF");
  view.setUint32(4, 36 + dataLen, true);
  writeStr(8, "WAVE");
  writeStr(12, "fmt ");
  view.setUint32(16, 16, true); // fmt chunk 大小
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // 單聲道
  view.setUint32(24, TARGET_RATE, true);
  view.setUint32(28, TARGET_RATE * 2, true); // byte rate
  view.setUint16(32, 2, true); // block align
  view.setUint16(34, 16, true); // bits per sample
  writeStr(36, "data");
  view.setUint32(40, dataLen, true);
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(44 + i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true);
  }
  return new Uint8Array(buf);
}
