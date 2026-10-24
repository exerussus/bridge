// Всплывающие окошки над кнопкой: закрываются щелчком мимо или Esc. Ничего не сдвигают — лежат поверх.
// Содержимое — компонент Svelte (popover) или готовый HTML для модулей холста (popHtml, как в доске и показе).
import { mount, unmount, type Component } from 'svelte';

type Pop = HTMLDivElement & { kind?: string; anchor?: Element; _t?: any; _c?: any; _onclose?: () => void };
let POP: Pop | null = null;

export function popOpen() { return POP; }

export function closePop() {
  if (!POP) return;
  const p = POP;
  POP = null;
  clearInterval(p._t);
  try { p._onclose && p._onclose(); } catch {}
  if (p._c) { try { unmount(p._c); } catch {} }
  p.remove();
}

function place(p: Pop, anchor: Element) {
  const r = anchor.getBoundingClientRect(), w = p.offsetWidth, h = p.offsetHeight;
  const left = Math.max(8, Math.min(innerWidth - w - 8, r.left + r.width / 2 - w / 2));
  const top = r.top - h - 10 >= 8 ? r.top - h - 10 : Math.min(innerHeight - h - 8, r.bottom + 10);
  p.style.left = left + 'px';
  p.style.top = top + 'px';
}

function open(anchor: Element, kind: string): Pop | null {
  if (POP && POP.kind === kind && POP.anchor === anchor) { closePop(); return null; }
  closePop();
  const p = document.createElement('div') as Pop;
  p.className = 'pop';
  p.kind = kind;
  p.anchor = anchor;
  (document.fullscreenElement || document.body).appendChild(p); // во весь экран — внутри него, иначе не видно
  POP = p;
  return p;
}

/** Окошко с компонентом. Повторный щелчок по той же кнопке закрывает его. */
export function popover<P extends Record<string, any>>(anchor: Element, kind: string, c: Component<P>, props: P = {} as P, style = ''): Pop | null {
  const p = open(anchor, kind);
  if (!p) return null;
  if (style) p.setAttribute('style', style);
  p._c = mount(c, { target: p, props });
  place(p, anchor);
  // содержимое может дорисоваться (списки устройств) — поправить положение
  requestAnimationFrame(() => { if (POP === p) place(p, anchor); });
  return p;
}

/** Окошко с готовым HTML (для холста доски и показа). mount — навесить обработчики. */
export function popHtml(anchor: Element, kind: string, html: string, mountFn?: (p: Pop) => void): Pop | null {
  const p = open(anchor, kind);
  if (!p) return null;
  p.innerHTML = html;
  place(p, anchor);
  mountFn && mountFn(p);
  return p;
}

document.addEventListener('pointerdown', e => {
  if (POP && !POP.contains(e.target as Node) && !POP.anchor!.contains(e.target as Node)) closePop();
}, true);
document.addEventListener('keydown', e => { if (e.key === 'Escape' && POP) closePop(); });
