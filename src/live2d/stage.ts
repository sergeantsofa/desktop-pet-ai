/**
 * Live2D 舞台:pixi 初始化、模型載入、互動反應、閒置動作。
 * M0:渲染、點擊/懸停反應、閒置小動作、視線追蹤。
 * M1:情緒標籤 → 表情/動作映射(setEmotion)。
 * M2:TTS 口型同步(setTalking;Web Speech 拿不到音訊波形,以正弦開合近似。
 *     之後換 Piper 產出音檔時,可改走 model.speak() 取得真實對嘴)。
 */
import { Application, Ticker } from "pixi.js";
import { Live2DModel } from "pixi-live2d-display-lipsyncpatch/cubism4";

export interface ActiveModelConfig {
  /** model3.json 路徑,相對於 public/,例如 "/models/Hiyori/Hiyori.model3.json" */
  path: string;
  /** 使用者縮放倍率(之後由設定頁控制) */
  scale?: number;
  /** 閒置幾分鐘後做小動作 */
  idleMinutes?: number;
  /**
   * 情緒標籤 → 表情/動作映射(規格 §4.9)。
   * 值為 expression 名稱,或 "motion:群組名"。
   * 例:{ "happy": "F01", "surprised": "motion:Flick" }
   */
  emotions?: Record<string, string>;
  /**
   * 每幀強制設定的參數,用來關掉 VTube 匯出模型多餘的半透明部件。
   * 這類參數常不被任何動作控制、卡在內建初始值,且設一次會被動畫每幀重置,
   * 所以必須每幀覆寫。例:{ "ShouBing": 0 } 關掉「手柄」那隻多餘的手。
   */
  fixedParams?: Record<string, number>;
}

export interface StageCallbacks {
  /** 角色想說話時(閒置碎念、點擊台詞) */
  onSay?: (line: string) => void;
}

const TAP_BODY_LINES = [
  "呀!幹嘛戳我啦~",
  "嗯?找我有事嗎?",
  "嘿嘿,我在喔。",
  "再戳我要生氣囉!(才不會)",
  "想聊天的話按 Ctrl+Shift+A 喔!",
];

const TAP_HEAD_LINES = ["嗯~摸頭好舒服…", "欸嘿嘿…", "頭髮要亂了啦~"];

const IDLE_LINES = ["呼啊…有點睏了…", "(東張西望)", "今天過得還好嗎?", "…zzZ"];

const HEAD_AREA = /head|face|頭/i;

let app: Application | null = null;
let model: Live2DModel | null = null;
let callbacks: StageCallbacks = {};
let userScale = 1;
let idleMs = 3 * 60_000;
let lastInteraction = Date.now();
let idleInterval: number | undefined;
let lastHoverReact = 0;
let emotionMap: Record<string, string> = {};
let fixedParams: Record<string, number> = {};

let globalListenersReady = false;

export function initStage(canvas: HTMLCanvasElement): void {
  app = new Application({
    view: canvas,
    backgroundAlpha: 0,
    resizeTo: window,
    autoDensity: true,
    resolution: window.devicePixelRatio || 1,
    antialias: true,
  });
}

/** 全域監聽只註冊一次(切換角色時靠 `model` 變數轉向,不重複掛) */
function ensureGlobalListeners(): void {
  if (globalListenersReady) return;
  globalListenersReady = true;
  window.addEventListener("resize", fitModel);
  window.addEventListener("pointermove", (e) => {
    model?.focus(e.clientX, e.clientY);
    handleHover(e.clientX, e.clientY);
  });
  watchDpr(); // 跨不同縮放比的螢幕時更新渲染解析度
}

/**
 * 視窗被拖到不同 DPI(螢幕縮放)的螢幕時,devicePixelRatio 會改變,
 * 但 pixi 的 renderer.resolution 是初始化時固定的,不更新就會大小錯亂、累積偏差。
 * 用 matchMedia 監聽 DPI 變化,變了就同步 resolution 並重新佈局。
 */
function watchDpr(): void {
  const dpr = window.devicePixelRatio || 1;
  const mq = window.matchMedia(`(resolution: ${dpr}dppx)`);
  mq.addEventListener(
    "change",
    () => {
      applyResolution();
      watchDpr(); // dpr 已變,改聽新的門檻
    },
    { once: true }
  );
}

function applyResolution(): void {
  if (!app) return;
  const dpr = window.devicePixelRatio || 1;
  // pixi v7 型別把 resolution 標記唯讀,但執行期可設;設完重新依視窗尺寸佈局
  (app.renderer as unknown as { resolution: number }).resolution = dpr;
  app.resize();
  fitModel();
}

/** 依指定設定載入模型(供首次載入與切換角色共用)。失敗時 throw。 */
export async function loadModel(cfg: ActiveModelConfig, cb?: StageCallbacks): Promise<void> {
  if (cb) callbacks = cb;
  if (!app) throw new Error("stage 尚未初始化");
  if (!window.Live2DCubismCore) {
    throw new Error(
      "找不到 Cubism Core。請從 live2d.com 下載 Web SDK,把 live2dcubismcore.min.js 放到「資料夾」按鈕開啟的 vendor 資料夾(開發者可放 public/vendor/)。"
    );
  }

  // 切換時先卸載舊模型
  if (model) {
    app.stage.removeChild(model as any);
    model.destroy();
    model = null;
  }

  userScale = cfg.scale ?? 1;
  idleMs = (cfg.idleMinutes ?? 3) * 60_000;
  emotionMap = cfg.emotions ?? {};
  fixedParams = cfg.fixedParams ?? {};

  const next = await Live2DModel.from(cfg.path, { ticker: Ticker.shared });
  model = next;
  app.stage.addChild(next as any);
  hookMouth();
  fitModel();
  ensureGlobalListeners();
  startIdleWatcher();
  markInteraction();
}

/** 讀取角色清單並載入當前選擇的角色。失敗時 throw,訊息給引導畫面用。 */
export async function loadActiveModel(cb: StageCallbacks = {}): Promise<void> {
  callbacks = cb;
  const { getActiveCharacter } = await import("./characters");
  const character = await getActiveCharacter();
  if (!character) {
    throw new Error(
      "找不到模型。請把 Live2D 模型放到「資料夾」按鈕開啟的 models 資料夾,並建立 characters.json(可參考 characters.example.json)。開發者也可放 public/models/。"
    );
  }
  await loadModel(character, cb);
}

/** 切換到指定角色設定(沿用既有的 onSay 等 callbacks)。 */
export async function switchModel(cfg: ActiveModelConfig): Promise<void> {
  await loadModel(cfg);
}

export function isModelLoaded(): boolean {
  return model !== null;
}

function fitModel(): void {
  if (!app || !model) return;
  const w = app.renderer.width / app.renderer.resolution;
  const h = app.renderer.height / app.renderer.resolution;
  // 以視窗高度為基準縮放,底部置中
  const scale = (h / model.internalModel.height) * 0.95 * userScale;
  model.scale.set(scale);
  model.anchor.set(0.5, 1);
  model.position.set(w / 2, h);
}

/** 點擊反應。回傳 true 代表點中角色(上層據此決定是否顯示台詞)。 */
export function handleTap(x: number, y: number): boolean {
  if (!model) return false;
  markInteraction();

  const areas = model.hitTest(x, y);
  const inBounds = model.getBounds().contains(x, y);
  if (areas.length === 0 && !inBounds) return false;

  if (areas.some((a) => HEAD_AREA.test(a))) {
    playRandomExpression();
    callbacks.onSay?.(pick(TAP_HEAD_LINES));
  } else {
    playRandomMotion();
    callbacks.onSay?.(pick(TAP_BODY_LINES));
  }
  return true;
}

/** 懸停頭部 → 表情反應(節流 5 秒一次) */
function handleHover(x: number, y: number): void {
  if (!model) return;
  const now = Date.now();
  if (now - lastHoverReact < 5_000) return;
  if (model.hitTest(x, y).some((a) => HEAD_AREA.test(a))) {
    lastHoverReact = now;
    playRandomExpression();
  }
}

/** 命中測試:座標是否在角色範圍內(供點擊穿透判斷等用途) */
export function hitsModel(x: number, y: number): boolean {
  if (!model) return false;
  return model.hitTest(x, y).length > 0 || model.getBounds().contains(x, y);
}

export function markInteraction(): void {
  lastInteraction = Date.now();
}

function startIdleWatcher(): void {
  if (idleInterval) window.clearInterval(idleInterval);
  idleInterval = window.setInterval(() => {
    if (Date.now() - lastInteraction >= idleMs) {
      markInteraction(); // 重置計時,避免連續觸發
      playRandomMotion();
      // 三成機率碎念一句
      if (Math.random() < 0.3) callbacks.onSay?.(pick(IDLE_LINES));
    }
  }, 30_000);
}

function playRandomMotion(): void {
  if (!model) return;
  const groups = Object.keys(
    (model.internalModel.motionManager.definitions ?? {}) as Record<string, unknown>
  ).filter((g) => (model!.internalModel.motionManager.definitions as any)[g]?.length);
  if (groups.length === 0) return;
  void model.motion(pick(groups));
}

function playRandomExpression(): void {
  if (!model) return;
  const mgr = model.internalModel.motionManager.expressionManager;
  if (!mgr || mgr.definitions.length === 0) {
    playRandomMotion();
    return;
  }
  void model.expression(Math.floor(Math.random() * mgr.definitions.length));
}

/**
 * 依情緒標籤切換表情/動作(M1,規格 §4.2)。
 * 優先用 active.json 的 emotions 映射;否則嘗試同名 expression;
 * 再不行就以隨機表情近似(neutral 不動作)。
 */
export function setEmotion(tag: string): void {
  if (!model) return;
  const mapped = emotionMap[tag];
  if (mapped) {
    if (mapped.startsWith("motion:")) {
      void model.motion(mapped.slice("motion:".length));
    } else {
      void model.expression(mapped);
    }
    return;
  }
  const mgr = model.internalModel.motionManager.expressionManager;
  if (mgr) {
    const idx = mgr.definitions.findIndex((d) => {
      const name = (d as any)?.Name ?? (d as any)?.name ?? "";
      return String(name).toLowerCase().includes(tag.toLowerCase());
    });
    if (idx >= 0) {
      void model.expression(idx);
      return;
    }
  }
  if (tag !== "neutral") playRandomExpression();
}

/* ---------------- 真實對嘴播放(M2 後半,Piper 音檔) ---------------- */

interface LipsyncOptions {
  volume?: number;
  onFinish?: () => void;
  onError?: () => void;
}

/**
 * 用 lipsyncpatch 的 model.speak() 播放音訊並驅動口型。
 * 回傳 false 代表模型未載入或不支援,呼叫端自行播放音訊。
 */
export function speakWithLipsync(url: string, opts: LipsyncOptions = {}): boolean {
  if (!model) return false;
  const m = model as unknown as {
    speak?: (
      url: string,
      options: {
        volume?: number;
        crossOrigin?: string;
        onFinish?: () => void;
        onError?: (err: unknown) => void;
      }
    ) => void;
  };
  if (typeof m.speak !== "function") return false;
  try {
    m.speak(url, {
      volume: opts.volume,
      crossOrigin: "anonymous",
      onFinish: opts.onFinish,
      onError: () => opts.onError?.(),
    });
    return true;
  } catch {
    return false;
  }
}

/** 停止 model.speak 的播放與口型 */
export function stopLipsync(): void {
  const m = model as unknown as { stopSpeaking?: () => void } | null;
  try {
    m?.stopSpeaking?.();
  } catch {
    /* 沒在播放時忽略 */
  }
}

/* ---------------- 口型同步(M2) ---------------- */

let talking = false;
let mouthNeedsClose = false;

/** TTS 說話中 → 嘴巴開合;結束時自然閉上。 */
export function setTalking(active: boolean): void {
  if (talking && !active) mouthNeedsClose = true;
  talking = active;
}

/**
 * 在 motionManager.update 之後覆寫參數:① 對嘴 ② fixedParams(關掉多餘部件)。
 * 動作曲線每幀都會重寫參數,所以必須掛在它後面才不會被蓋掉。
 */
function hookMouth(): void {
  if (!model) return;
  const mm = model.internalModel.motionManager as unknown as {
    update: (core: object, now: number) => boolean;
  };
  const original = mm.update.bind(mm);
  mm.update = (core, now) => {
    const updated = original(core, now);
    const setParam = (core as { setParameterValueById?: (id: string, v: number) => void })
      .setParameterValueById;
    if (typeof setParam !== "function") return updated;
    try {
      // 每幀強制固定參數(道具/多餘部件),避免被動畫重置回半透明
      for (const id in fixedParams) {
        setParam.call(core, id, fixedParams[id]);
      }
      if (talking) {
        // 兩個不同週期的正弦疊加,看起來比單一頻率自然
        const t = now / 1000;
        const v = Math.max(0, Math.abs(Math.sin(t * 9)) * 0.6 + Math.sin(t * 23) * 0.25);
        setParam.call(core, "ParamMouthOpenY", Math.min(1, v));
      } else if (mouthNeedsClose) {
        mouthNeedsClose = false;
        setParam.call(core, "ParamMouthOpenY", 0);
      }
    } catch {
      /* 模型缺對應參數也不致命 */
    }
    return updated;
  };
}

function pick<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
}
