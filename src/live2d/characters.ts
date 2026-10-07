/**
 * 多角色管理(角色清單 + 當前選擇)。
 * 來源優先序:
 *   1. 外部 %APPDATA%\com.desktoppet.ai\models\characters.json(發行版使用者放這;path 相對 models 資料夾)
 *   2. 打包 /models/characters.json(開發者放 public/)
 *   3. 舊的單一 active.json(向後相容)
 * 當前選擇記在 localStorage,沒選過時用 manifest 的 active 欄位(再退回第一個)。
 */
import type { ActiveModelConfig } from "./stage";
import { resourceBase } from "../resources";

export interface Character extends ActiveModelConfig {
  id: string;
  name: string;
}

interface Manifest {
  active?: string;
  characters: Character[];
}

const SELECTED_KEY = "selected-character";

let cache: Character[] | null = null;
let manifestActive = "";

/** 試讀外部資源伺服器的角色清單;path 轉成 http URL。沒有回 null。 */
async function loadExternalCharacters(): Promise<Character[] | null> {
  const base = await resourceBase();
  if (!base) return null;
  try {
    const res = await fetch(`${base}/models/characters.json`);
    if (!res.ok) return null;
    const m = (await res.json()) as Manifest;
    if (!Array.isArray(m.characters) || m.characters.length === 0) return null;
    manifestActive = m.active ?? m.characters[0].id;
    // path 可能是兩種寫法,兩種都要支援:
    //   "icegirl/IceGirl.model3.json"          → 相對 models 資料夾
    //   "/models/icegirl/IceGirl.model3.json"  → 已是從伺服器根算起的完整路徑
    // (自己組的 characters.json 常用後者;以前一律接 `${base}/models/`,
    //  遇到後者會變成 /models/models/... → 404 → 變成「有清單卻沒角色」)
    for (const c of m.characters) {
      const p = c.path.replace(/^[/\\]+/, "");
      c.path = p.startsWith("models/") ? `${base}/${p}` : `${base}/models/${p}`;
    }
    return m.characters;
  } catch {
    return null;
  }
}

/** 載入角色清單(快取):外部 appdata 優先,再退回打包 /models,最後 active.json。 */
export async function loadCharacters(): Promise<Character[]> {
  if (cache) return cache;
  const ext = await loadExternalCharacters();
  if (ext) {
    cache = ext;
    return cache;
  }
  const res = await fetch("/models/characters.json").catch(() => null);
  if (res && res.ok) {
    const m = (await res.json()) as Manifest;
    if (Array.isArray(m.characters) && m.characters.length > 0) {
      manifestActive = m.active ?? m.characters[0].id;
      cache = m.characters;
      return cache;
    }
  }
  // 向後相容:沒有 characters.json → 用舊的 active.json
  const legacy = await fetch("/models/active.json").catch(() => null);
  if (legacy && legacy.ok) {
    const cfg = (await legacy.json()) as ActiveModelConfig;
    cache = [{ id: "default", name: "預設角色", ...cfg }];
    manifestActive = "default";
    return cache;
  }
  cache = [];
  return cache;
}

/** 取得當前要顯示的角色(localStorage > manifest.active > 第一個)。 */
export async function getActiveCharacter(): Promise<Character | null> {
  const list = await loadCharacters();
  if (list.length === 0) return null;
  const saved = localStorage.getItem(SELECTED_KEY);
  return (
    list.find((c) => c.id === saved) ??
    list.find((c) => c.id === manifestActive) ??
    list[0]
  );
}

export function setActiveCharacterId(id: string): void {
  localStorage.setItem(SELECTED_KEY, id);
}

export async function getActiveCharacterId(): Promise<string> {
  return (await getActiveCharacter())?.id ?? "";
}
