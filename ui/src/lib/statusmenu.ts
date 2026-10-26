// Быстрый выбор статуса: правая кнопка по своей плитке — «Без статуса» или одна из заготовок (иконка и текст).
import { api, aurl } from './api';
import { toast } from './state.svelte';
import { esc } from './util';
import { loadAvatars } from '../voice/engine.svelte';
import { goSettings } from './nav';
import { hideStatusPop } from './statuspop';

let menu: HTMLDivElement | null = null;
function close() { if (menu) { menu.remove(); menu = null; document.removeEventListener('pointerdown', outside, true); document.removeEventListener('keydown', key, true); } }
const outside = (e: Event) => { if (menu && !menu.contains(e.target as Node)) close(); };
const key = (e: KeyboardEvent) => { if (e.key === 'Escape') close(); };

export async function openStatusMenu(e: MouseEvent) {
  e.preventDefault(); close(); hideStatusPop();
  let r: any;
  try { r = await api('GET', 'tile/status/presets'); } catch (er: any) { toast('Статусы: ' + er.message, 'bad'); return; }
  const icon = (p: any) => (p.img ? '<span class="sticon"><img src="' + esc(aurl('tile/status/preset/image?id=' + encodeURIComponent(p.id) + '&v=' + p.img)) + '" alt=""></span>' : '<span class="sticon" style="background:' + esc(p.color || 'transparent') + '"></span>');
  const items = r.list.filter((p: any) => p.text.trim());
  menu = document.createElement('div'); menu.className = 'stmenu'; menu.setAttribute('role', 'menu');
  menu.innerHTML = '<div class="stmh">Статус</div>' +
    '<button role="menuitem" data-id=""' + (r.active ? '' : ' class="on"') + '><span class="sticon nost"></span><span class="t">Без статуса</span></button>' +
    items.map((p: any) => '<button role="menuitem" data-id="' + esc(p.id) + '"' + (r.active === p.id ? ' class="on"' : '') + '>' + icon(p) + '<span class="t">' + esc(p.text) + '</span>' + (p.silent ? '<span class="stmsil" title="Полная тишина">🔕</span>' : '') + '</button>').join('') +
    (items.length ? '' : '<div class="stmn">Заготовок пока нет</div>') +
    '<div class="stmsep"></div><button role="menuitem" data-cfg="1"><span class="sticon nost"></span><span class="t">Настроить статусы…</span></button>';
  document.body.appendChild(menu);
  const w = menu.offsetWidth, h = menu.offsetHeight;
  menu.style.left = Math.min(innerWidth - w - 8, e.clientX + 2) + 'px';
  menu.style.top = Math.min(innerHeight - h - 8, e.clientY + 2) + 'px';
  menu.addEventListener('click', async ev => {
    const b = (ev.target as HTMLElement).closest('button'); if (!b) return;
    close();
    if (b.dataset.cfg) { goSettings('tile'); return; }
    try { await api('POST', 'tile/status/apply', { id: b.dataset.id || '' }); await loadAvatars(); toast(b.dataset.id ? 'Статус выставлен' : 'Без статуса', 'ok'); }
    catch (er: any) { toast(er.message, 'bad'); }
  });
  setTimeout(() => { document.addEventListener('pointerdown', outside, true); document.addEventListener('keydown', key, true); }, 0);
}
