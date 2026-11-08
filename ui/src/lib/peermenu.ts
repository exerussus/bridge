// (3.21.2) Правая кнопка по чужой плитке в канале: «Переподключить» (соединение голоса с нуля у обоих) и «Написать» (личный чат).
import { toast, titleOf, setTab } from './state.svelte';
import { esc } from './util';
import { CHT, chLoadList } from './chat.svelte';
import { relinkPeer, connState } from '../voice/engine.svelte';
import { hideStatusPop } from './statuspop';

let menu: HTMLDivElement | null = null;
function close() { if (menu) { menu.remove(); menu = null; document.removeEventListener('pointerdown', outside, true); document.removeEventListener('keydown', key, true); } }
const outside = (e: Event) => { if (menu && !menu.contains(e.target as Node)) close(); };
const key = (e: KeyboardEvent) => { if (e.key === 'Escape') close(); };

export async function openChatWith(name: string) {
  const conv = 'dm:' + name;
  if (!CHT.list.some(c => c.conv === conv)) await chLoadList();
  CHT.cur = conv;
  setTab('chat');
  setTimeout(() => (document.querySelector('#t-chat .chatbox textarea') as HTMLElement | null)?.focus(), 60);
}

export function openPeerMenu(e: MouseEvent, name: string) {
  e.preventDefault(); close(); hideStatusPop();
  const cs = connState(name).cs;
  menu = document.createElement('div'); menu.className = 'stmenu'; menu.setAttribute('role', 'menu');
  menu.innerHTML = '<div class="stmh">' + esc(titleOf(name)) + '</div>' +
    '<button role="menuitem" data-a="relink" title="Соединение голоса и показов с этим человеком создаётся с нуля у обоих — если его не слышно или он вас не слышит"><span class="pmi">⟳</span><span class="t">Переподключить' + (cs ? ' <span class="muted small">(' + esc(cs) + ')</span>' : '') + '</span></button>' +
    '<button role="menuitem" data-a="chat"><span class="pmi">✉</span><span class="t">Написать</span></button>';
  document.body.appendChild(menu);
  const w = menu.offsetWidth, h = menu.offsetHeight;
  menu.style.left = Math.min(innerWidth - w - 8, e.clientX + 2) + 'px';
  menu.style.top = Math.min(innerHeight - h - 8, e.clientY + 2) + 'px';
  menu.addEventListener('click', async ev => {
    const b = (ev.target as HTMLElement).closest('button'); if (!b) return;
    close();
    if (b.dataset.a === 'chat') { openChatWith(name); return; }
    if (await relinkPeer(name)) toast('Переподключаюсь к ' + titleOf(name) + '…');
    else toast(titleOf(name) + ' сейчас не в канале', 'warn');
  });
  setTimeout(() => { document.addEventListener('pointerdown', outside, true); document.addEventListener('keydown', key, true); }, 0);
}
