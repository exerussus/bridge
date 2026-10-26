// Плитка участника: стиль (фон-картинка, маска, прозрачность, оттенок, аватарка, цвета) — свой и соседей.
// Мост хранит свою плитку и забирает соседские по отпечатку (как аватарки); здесь — кэш и превращение стиля в CSS.
import { api, aurl } from './api';
import { me as ME } from './state.svelte';
import { AV } from '../voice/engine.svelte';

export type TileStyle = {
  fit?: 'cover' | 'contain' | 'repeat'; x?: number; y?: number; zoom?: number;
  mask?: 'none' | 'bottom' | 'top' | 'sides' | 'vignette' | 'circle' | 'diagonal'; soft?: number;
  opacity?: number; blur?: number; tint?: string; tintAmt?: number; base?: string; border?: string;
  avShape?: 'circle' | 'round' | 'square' | 'hex' | 'none'; avSize?: number; avRing?: string; glow?: string; name?: string;
  /** (3.3.1) статус: текст до 255 знаков и цвет значка */
  stText?: string; stColor?: string; stSilent?: boolean;
};
export const TILE_DEFAULT: Required<Omit<TileStyle, 'tint' | 'base' | 'border' | 'avRing' | 'glow' | 'name' | 'stText' | 'stColor' | 'stSilent'>> = { fit: 'cover', x: 50, y: 50, zoom: 100, mask: 'bottom', soft: 60, opacity: 70, blur: 0, tintAmt: 0, avShape: 'circle', avSize: 100 };

type Tile = { style: TileStyle; img: string; h: string; stImg?: string };
export const TILES = $state<Record<string, Tile>>({});
const loading = new Set<string>();

/** Плитка участника (реактивно): подгружает при смене отпечатка. null — обычная. */
export function tileOf(n: string): Tile | null {
  const mine = n === ME(), h = mine ? AV.tile : AV.tiles[n];
  if (!h) return null;
  const t = TILES[n];
  if ((!t || t.h !== h) && !loading.has(n + h)) {
    loading.add(n + h);
    api('GET', 'tile?peer=' + encodeURIComponent(mine ? '' : n)).then(r => { TILES[n] = { style: r.style || {}, img: r.img || '', stImg: r.stImg || '', h }; }).catch(() => {}).finally(() => loading.delete(n + h));
  }
  return t && t.h === h ? t : t || null;
}
export const stImgUrl = (n: string, h: string) => (h ? aurl('tile/status/image?peer=' + encodeURIComponent(n === ME() ? '' : n) + '&v=' + h) : '');
export const tileImgUrl = (n: string, img: string) => (img ? aurl('tile/image?peer=' + encodeURIComponent(n === ME() ? '' : n) + '&v=' + img) : '');

/** Маска картинки-фона (CSS mask-image). */
export function maskOf(st: TileStyle): string {
  const m = st.mask ?? TILE_DEFAULT.mask, k = 8 + (st.soft ?? TILE_DEFAULT.soft) * 0.8;
  switch (m) {
    case 'bottom': return `linear-gradient(to bottom,#000 ${Math.max(0, 100 - k)}%,transparent 100%)`;
    case 'top': return `linear-gradient(to top,#000 ${Math.max(0, 100 - k)}%,transparent 100%)`;
    case 'sides': return `linear-gradient(to right,transparent 0,#000 ${k / 2}%,#000 ${100 - k / 2}%,transparent 100%)`;
    case 'vignette': return `radial-gradient(ellipse 72% 72% at 50% 50%,#000 ${Math.max(0, 100 - k * 1.1)}%,transparent 100%)`;
    case 'circle': return `radial-gradient(circle closest-side at 50% 50%,#000 ${Math.max(0, 100 - k)}%,transparent 100%)`;
    case 'diagonal': return `linear-gradient(135deg,#000 ${Math.max(0, 50 - k / 2)}%,transparent ${Math.min(100, 50 + k / 2)}%)`;
    default: return 'none';
  }
}
/** Переменные и классы для самой плитки: цвета рамки, фона, подсветки, имени; форма и размер аватарки. */
export function tileVars(st: TileStyle | null | undefined): string {
  if (!st) return '';
  const v: string[] = [];
  if (st.base) v.push('background-color:' + st.base);
  if (st.border) v.push('--tb:' + st.border);
  if (st.glow) v.push('--spk:' + st.glow);
  if (st.name) v.push('--tn:' + st.name);
  if (st.avRing) v.push('--ring:' + st.avRing);
  v.push('--avk:' + (st.avSize ?? 100) / 100);
  return v.join(';');
}
export const tileCls = (st: TileStyle | null | undefined) => (st ? 'tcust sh-' + (st.avShape || 'circle') + (st.border ? ' tbord' : '') + (st.avRing ? ' tring' : '') + (st.glow ? ' tglow' : '') + (st.name ? ' tname' : '') : '');
