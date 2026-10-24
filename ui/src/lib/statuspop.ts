// Статус участника: при наведении на плитку (на пустое место, не на кнопки) — окошко ниже и правее мыши.
import { titleOf, me } from './state.svelte';
import { esc } from './util';

export type StatusInfo = { name: string; text?: string; color?: string; img?: string } | null;
let pop: HTMLDivElement | null = null;
function hide() { if (pop) { pop.remove(); pop = null; } }

export function statusHover(node: HTMLElement, info: StatusInfo) {
  let st = info;
  const move = (e: PointerEvent) => {
    const has = st && (st.text || st.color || st.img);
    if (!has || (e.target as HTMLElement).closest('button, a, input, video, [role="button"]')) { hide(); return; }
    if (!pop) {
      pop = document.createElement('div'); pop.className = 'stpop'; pop.setAttribute('role', 'tooltip');
      document.body.appendChild(pop);
    }
    const ic = st!.img ? '<span class="sticon"><img src="' + esc(st!.img) + '" alt=""></span>' : st!.color ? '<span class="sticon" style="background:' + esc(st!.color) + '"></span>' : '';
    const html = ic + '<div><b>' + esc(st!.name === me() ? 'Ваш статус' : titleOf(st!.name)) + '</b><span class="t">' + esc(st!.text || '') + '</span></div>';
    if (pop.dataset.h !== html) { pop.innerHTML = html; pop.dataset.h = html; }
    const w = pop.offsetWidth, h = pop.offsetHeight;
    pop.style.left = Math.min(innerWidth - w - 8, e.clientX + 14) + 'px';
    pop.style.top = (e.clientY + 18 + h > innerHeight - 8 ? e.clientY - h - 10 : e.clientY + 18) + 'px';
  };
  node.addEventListener('pointermove', move);
  node.addEventListener('pointerleave', hide);
  return {
    update(v: StatusInfo) { st = v; },
    destroy() { node.removeEventListener('pointermove', move); node.removeEventListener('pointerleave', hide); hide(); },
  };
}
