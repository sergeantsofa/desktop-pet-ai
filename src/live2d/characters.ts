/**
 * 多角色管理(角色清單 + 當前選擇)。
 * 角色清單放 public/models/characters.json;當前選擇記在 localStorage,
 * 沒選過時用 manifest 的 active 欄位(再退回清單第一個)。
 * 找不到 characters.json 時退回舊的單一 active.json(向後相容)。
 */
import type { ActiveModelConfig } from "./stage";

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

/** 載入角色清單(快取);失敗時退回 active.json 當單一角色。 */
export async function loadCharacters(): Promise<Character[]> {
  if (cache) return cache;
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
