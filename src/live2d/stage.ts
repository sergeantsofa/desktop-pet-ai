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
  "喂喂,手放開啦,很癢欸!",
  "戳夠了沒~人家又不是泡泡紙。",
  "你是不是很無聊?要不要陪我聊天?",
  "哼,只有想戳我的時候才想起我喔?",
  "摸魚被我看到囉~快回去工作啦!",
  "再戳…我可是會記仇的喔(小聲)",
  "想聊天的話按 Ctrl+Shift+A 喔!",
];

const TAP_HEAD_LINES = [
  "嗯~摸頭好舒服…",
  "欸嘿嘿…",
  "頭髮要亂了啦~",
  "再多摸一下下嘛…",
  "呼…被摸頭整個人都軟了。",
  "你的手好溫暖喔…",
  "哼,勉強讓你摸一下啦。",
];

const IDLE_LINES = [
  "呼啊…有點睏了…",
  "(東張西望)",
  "今天過得還好嗎?",
  "…zzZ",
  "好安靜喔…大家都在忙嗎?",
  "無聊死了啦,陪我玩咩~",
  "我在這裡乖乖等你喔。",
  "剛剛那個…算了,沒事。",
  "(偷偷看了你一眼)",
  "要不要喝口水、休息一下?",
  "嗯…肚子有點餓了呢。",
];

/** 點頭(摸頭)→ 開心的表情;點身體 → 害羞的表情(明確情緒,不亂數) */
const HEAD_REACT_EXPR = ["爱心眼", "星星眼"];
const BODY_REACT_EXPR = ["脸红"];

const HEAD_AREA = /head|face|頭/i;

let app: Application | null = null;
let model: Live2DModel | null = null;
let callbacks: StageCallbacks = {};
/** 角色基準縮放(來自 characters.json 的 scale,切換角色時更新) */
let baseScale = 1;
/** 使用者額外縮放倍率(設定面板滑桿;跨角色切換保留) */
let userScale = 1;
let idleMs = 3 * 60_000;
let lastInteraction = Date.now();
let idleInterval: number | undefined;
let lastHoverReact = 0;
let emotionMap: Record<string, string> = {};
let fixedParams: Record<string, number> = {};

/** 頭部中心約在模型整體高度「由上往下」這個比例處,當作視線的平視基準點 */
const HEAD_GAZE_FRAC = 0.18;
/** 視線垂直偏移:把基準從「模型幾何中心」上移到「頭部中心」(隨佈局更新) */
let gazeYOffset = 0;

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
    gazeAt(e.clientX, e.clientY);
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

  baseScale = cfg.scale ?? 1;
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
  // 以視窗高度為基準縮放,底部置中;再乘上角色基準與使用者倍率
  const scale = (h / model.internalModel.height) * 0.95 * baseScale * userScale;
  model.scale.set(scale);
  model.anchor.set(0.5, 1);
  model.position.set(w / 2, h);
  updateGazeOffset();
}

/**
 * 重算視線垂直偏移。`model.focus()` 內部以模型幾何中心(高度 50%)為平視基準,
 * 但頭部在上方(整體高度約 18% 處),所以游標在頭旁邊時會被算成「一直往上看」。
 * 這裡算出「中心 → 頭部」的螢幕距離,之後把游標 y 加上它,讓游標在頭部高度時視線為平視。
 */
function updateGazeOffset(): void {
  if (!model) return;
  const b = model.getBounds();
  gazeYOffset = b.height * (0.5 - HEAD_GAZE_FRAC);
}

/** 讓模型看向螢幕座標 (x, y),但以頭部中心為基準(修正垂直偏移)。 */
function gazeAt(x: number, y: number): void {
  model?.focus(x, y + gazeYOffset);
}

/** 設定使用者額外縮放倍率(設定面板滑桿用);會即時重新佈局。 */
export function setUserScale(scale: number): void {
  userScale = Math.max(0.3, Math.min(3, scale));
  fitModel();
}

/** 取目前使用者縮放倍率。 */
export function getUserScale(): number {
  return userScale;
}

/**
 * 角色在畫面上的位置(CSS px):頭頂 y、水平中心 x。給對話泡泡跟著頭定位用。
 * 沒有模型時回 null(呼叫端退回預設位置)。
 */
export function getHeadAnchor(): { x: number; y: number } | null {
  if (!app || !model) return null;
  const b = model.getBounds();
  return { x: b.x + b.width / 2, y: b.y };
}

/** 點擊反應。回傳 true 代表點中角色(上層據此決定是否顯示台詞)。 */
export function handleTap(x: number, y: number): boolean {
  if (!model) return false;
  markInteraction();

  const areas = model.hitTest(x, y);
  const inBounds = model.getBounds().contains(x, y);
  if (areas.length === 0 && !inBounds) return false;

  if (areas.some((a) => HEAD_AREA.test(a))) {
    // 摸頭 → 開心表情 + 眨眼動作(MeiYan)
    playExpression(pick(HEAD_REACT_EXPR));
    playMotionGroup("Tap");
    callbacks.onSay?.(pick(TAP_HEAD_LINES));
  } else {
    // 點身體 → 害羞表情
    playExpression(pick(BODY_REACT_EXPR));
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
      // 閒置小動作:一半機率東張西望(視線飄移)、一半播待機動作
      if (Math.random() < 0.5) {
        lookAround();
      } else {
        playMotionGroup("Idle");
      }
      // 半數機率碎念一句
      if (Math.random() < 0.5) callbacks.onSay?.(pick(IDLE_LINES));
    }
  }, 30_000);
}

/** 視線飄到畫面內隨機一點,模擬「東張西望」 */
function lookAround(): void {
  if (!model) return;
  const x = window.innerWidth * (0.25 + Math.random() * 0.5);
  const y = window.innerHeight * (0.2 + Math.random() * 0.45);
  gazeAt(x, y);
}

function playRandomMotion(): void {
  if (!model) return;
  const groups = Object.keys(
    (model.internalModel.motionManager.definitions ?? {}) as Record<string, unknown>
  ).filter((g) => (model!.internalModel.motionManager.definitions as any)[g]?.length);
  if (groups.length === 0) return;
  void model.motion(pick(groups));
}

/** 依名稱播放表情(找不到就忽略,不報錯) */
function playExpression(name: string): void {
  if (!model) return;
  try {
    void model.expression(name);
  } catch {
    /* 該模型沒有這個表情就算了 */
  }
}

/** 播放指定動作組(該組沒動作就忽略) */
function playMotionGroup(group: string): void {
  if (!model) return;
  const defs = (model.internalModel.motionManager.definitions as any) ?? {};
  if (defs[group]?.length) void model.motion(group);
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
let swayActive = false;
let swayFreq = 0; // rad / ms
let swayNeedsReset = false;

/** TTS 說話中 → 嘴巴開合;結束時自然閉上。 */
export function setTalking(active: boolean): void {
  if (talking && !active) mouthNeedsClose = true;
  talking = active;
}

/**
 * 隨音樂搖擺(Spotify 播放中時)。bpm>0 就對到節奏(一次左右擺約兩拍),
 * 拿不到 bpm 就用預設 ~96。停止時把身體參數歸位。
 */
export function setSway(active: boolean, bpm = 0): void {
  if (swayActive && !active) swayNeedsReset = true;
  swayActive = active;
  if (active) {
    const eff = bpm > 0 ? bpm : 96;
    swayFreq = (Math.PI * eff) / 60 / 1000;
  }
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
      // 隨音樂搖擺:這個模型沒有 ParamBody*,改用頭部(轉+傾同相位)做出明顯的跟拍擺動
      if (swayActive) {
        const s = Math.sin(now * swayFreq);
        setParam.call(core, "ParamAngleX", s * 20); // 頭左右轉
        setParam.call(core, "ParamAngleZ", s * 16); // 頭左右傾(同相位 → 像在打拍子)
      } else if (swayNeedsReset) {
        swayNeedsReset = false;
        setParam.call(core, "ParamAngleX", 0);
        setParam.call(core, "ParamAngleZ", 0);
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
