// Появление новых сообщений: пузырь влетает со стороны отправителя, слова проявляются по одному, стикер выпрыгивает.
// Только новые, пока беседа открыта; история при открытии — сразу.
import { CHT, chMsgs } from './chat.svelte';

const SEEN: Record<string, Set<string>> = {};
export function seenFor(conv: string): Set<string> {
  return (SEEN[conv] = SEEN[conv] || new Set(chMsgs(conv).map(m => m.id)));
}

export function animMsg(m: HTMLElement, a: { id: string; conv: string }) {
  const seen = seenFor(a.conv);
  if (CHT.first || seen.has(a.id)) { seen.add(a.id); return; }
  seen.add(a.id);
  m.classList.add('anim');
  m.querySelectorAll<HTMLElement>('.pics img').forEach(i => { i.classList.add('anim'); i.style.animationDelay = '150ms'; });
  if (m.classList.contains('stk') || m.classList.contains('bigemo')) { setTimeout(() => m.classList.remove('anim'), 1200); return; }
  const nodes: Text[] = [];
  const w = document.createTreeWalker(m, NodeFilter.SHOW_TEXT, { acceptNode: n => (n.parentElement!.closest('.meta,.pics,.files') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT) });
  while (w.nextNode()) nodes.push(w.currentNode as Text);
  const total = nodes.reduce((s, n) => s + n.data.split(/\s+/).length, 0);
  let i = 0;
  if (total <= 90) {
    for (const n of nodes) {
      const frag = document.createDocumentFragment();
      for (const part of n.data.split(/(\s+)/)) {
        if (!part) continue;
        if (/^\s+$/.test(part)) { frag.append(part); continue; }
        const s = document.createElement('span');
        s.className = 'w'; s.textContent = part; s.style.animationDelay = 140 + i++ * 45 + 'ms';
        frag.append(s);
      }
      n.parentNode!.replaceChild(frag, n);
    }
  }
  m.querySelectorAll<HTMLElement>('.meta').forEach(x => { x.classList.add('anim'); x.style.animationDelay = 220 + i * 45 + 'ms'; });
  // анимация кончилась — обычное сообщение (переносы как у всех, кнопки над пузырём не обрезаются)
  setTimeout(() => { m.classList.remove('anim'); m.querySelectorAll('.anim').forEach(x => x.classList.remove('anim')); }, 1400 + i * 45);
}

/** Держать низ ленты внизу, пока читатель внизу: картинки догружаются, поле ввода растёт, колонку тянут. */
export function stick(feed: HTMLElement) {
  let on = true;
  const keep = () => { if (on) feed.scrollTop = feed.scrollHeight; };
  const onScroll = () => { on = feed.scrollHeight - feed.scrollTop - feed.clientHeight < 40; };
  feed.addEventListener('scroll', onScroll, { passive: true });
  const ro = new ResizeObserver(keep); ro.observe(feed);
  const mo = new MutationObserver(keep); mo.observe(feed, { childList: true, subtree: true });
  feed.addEventListener('load', keep, true);
  feed.addEventListener('animationend', keep, true);
  requestAnimationFrame(() => { feed.scrollTop = feed.scrollHeight; });
  (feed as any)._stickNow = () => { on = true; keep(); };
  return {
    destroy() { ro.disconnect(); mo.disconnect(); feed.removeEventListener('scroll', onScroll); feed.removeEventListener('load', keep, true); feed.removeEventListener('animationend', keep, true); },
  };
}
