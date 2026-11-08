<script lang="ts">
  // (3.21) Ctrl K: перейти к разделу или соседу, выполнить частое действие — с клавиатуры.
  import { ui, setTab, closeModal, HEAD, toast, load } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { toggleMic, toggleDeaf } from '../voice/engine.svelte';

  type It = { t: string; s: string; go: () => void };
  let q = $state('');
  let sel = $state(0);
  const items = $derived.by(() => {
    const L: It[] = [];
    for (const [k, h] of Object.entries(HEAD)) L.push({ t: h[0], s: 'раздел', go: () => setTab(k) });
    for (const p of ui.ov?.peers || []) L.push({ t: p.title || p.name, s: 'сосед · ' + (p.online ? 'на связи' : 'нет связи'), go: () => { ui.peerOpen = p.name; ui.peersSub = 'list'; setTab('peers'); } });
    L.push({ t: 'Обменяться со всеми', s: 'действие · S', go: async () => { try { await api('POST', 'sync'); toast('Обмен выполнен', 'ok'); load(); } catch (e: any) { toast(e.message, 'bad'); } } });
    L.push({ t: 'Микрофон: выключить / включить', s: 'действие · Ctrl M', go: toggleMic });
    L.push({ t: 'Звук: выключить / включить', s: 'действие · Ctrl D', go: toggleDeaf });
    const w = q.trim().toLowerCase();
    return w ? L.filter(x => (x.t + ' ' + x.s).toLowerCase().includes(w)) : L;
  });
  $effect(() => { void q; sel = 0; });
  function run(i: It | undefined) { if (!i) return; closeModal(); i.go(); }
  function key(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') { e.preventDefault(); sel = Math.min(items.length - 1, sel + 1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); sel = Math.max(0, sel - 1); }
    else if (e.key === 'Enter') { e.preventDefault(); run(items[sel]); }
  }
  function focus(n: HTMLInputElement) { setTimeout(() => n.focus(), 0); }
</script>

<div class="pal">
  <input use:focus bind:value={q} onkeydown={key} placeholder="Раздел, сосед или действие…" aria-label="Поиск" />
  <div class="pal-list">
    {#each items.slice(0, 40) as it, i}
      <button class:on={i === sel} onmouseenter={() => (sel = i)} onclick={() => run(it)}><span class="grow ell">{it.t}</span><span class="muted small">{it.s}</span></button>
    {:else}<div class="empty">Ничего не нашлось</div>{/each}
  </div>
</div>
