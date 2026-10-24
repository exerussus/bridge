<script lang="ts">
  // Сборки: папка программы уходит соседу одним zip и распаковывается у него.
  // Патч — только изменённое с прошлой отправки (с заменой); чистая — целиком, прежняя папка у соседа уходит в корзину.
  import { ui, toast, peerTitle, SET, setSet } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, size, fullTime } from '../lib/util';
  import Gi from './Gi.svelte';

  const peers = $derived(ui.ov.peers.filter((p: any) => p.enabled && !p.invite));
  let peer = $state(SET.buildPeer || '');
  $effect(() => { if (!peer || !peers.some((p: any) => p.name === peer)) peer = peers[0]?.name || ''; });
  let src = $state(SET.buildSrc || '');
  let name = $state(SET.buildName || '');
  let mode = $state<'patch' | 'clean'>(SET.buildMode === 'clean' ? 'clean' : 'patch');
  let B = $state<any>(null);
  let busy = $state(false);
  const guess = (p: string) => p.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || '';
  $effect(() => {
    let t: any = 0, live = true;
    const tick = async () => { try { B = await api('GET', 'builds'); } catch {} if (live) t = setTimeout(tick, B && B.jobs.length ? 1000 : 5000); };
    tick();
    return () => { live = false; clearTimeout(t); };
  });
  async function send() {
    const n = (name.trim() || guess(src)).replace(/[^\p{L}\p{N} ._-]/gu, '_');
    if (!src.trim()) { toast('Укажите папку сборки — полный путь', 'bad'); return; }
    busy = true;
    try {
      await api('POST', 'builds/send', { peer, path: src.trim(), name: n, mode });
      setSet('buildPeer', peer); setSet('buildSrc', src.trim()); setSet('buildName', n); setSet('buildMode', mode);
      name = n; toast('Упаковываю «' + n + '» для ' + peerTitle(peer) + '…');
      B = await api('GET', 'builds');
    } catch (e: any) { toast(e.message, 'bad'); }
    busy = false;
  }
  async function setTarget(r: any) {
    const v = prompt('Куда распаковывать сборку «' + r.name + '» от ' + peerTitle(r.peer) + '?\nПолный путь; пусто — по умолчанию (' + B.defaultRoot + ').', r.target);
    if (v === null) return;
    try { await api('POST', 'builds/target', { peer: r.peer, name: r.name, path: v.trim() }); toast('Следующая сборка распакуется сюда', 'ok'); B = await api('GET', 'builds'); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function openDir(r: any) { try { await api('POST', 'open?' + new URLSearchParams({ target: 'build', peer: r.peer, name: r.name })); } catch (e: any) { toast(e.message, 'bad'); } }
  const sent = $derived(B ? B.history.filter((h: any) => h.dir === 'out').slice(0, 8) : []);
</script>

<div class="card builds" style="margin-top:14px">
  <div class="row"><h3 style="margin:0"><Gi n="folder" /> Сборки — одним архивом</h3></div>
  <div class="small muted" style="margin:6px 0 10px">Папка программы (например, сборка Unity) уходит соседу одним zip и сама распаковывается у него. <b>Патч</b> — только файлы, изменившиеся с прошлой отправки, с заменой. <b>Чистая</b> — всё целиком: прежняя папка у соседа уходит в корзину, остаётся ровно присланное.</div>
  <div class="bgrid">
    <label class="f">Кому<select bind:value={peer}>{#each peers as p (p.name)}<option value={p.name}>{(p.title || p.name) + (p.online ? '' : ' (нет связи)')}</option>{/each}</select></label>
    <label class="f">Папка сборки на этом ПК<input bind:value={src} placeholder="D:\Unity\CORA\Build" oninput={() => { if (!name) name = ''; }} /></label>
    <label class="f">Название<input bind:value={name} placeholder={guess(src) || 'CORA'} /></label>
  </div>
  <div class="row" style="gap:10px;flex-wrap:wrap">
    <span class="seg"><button class:on={mode === 'patch'} onclick={() => (mode = 'patch')}>Патч — только изменённое</button><button class:on={mode === 'clean'} onclick={() => (mode = 'clean')}>Чистая — заменить целиком</button></span>
    <button class="b pri" disabled={busy || !peer} onclick={send}>Упаковать и отправить</button>
    <span class="small muted">Первая отправка сборки соседу всегда целиком.</span>
  </div>
  {#each B ? B.jobs : [] as j}
    <div style="margin-top:8px"><div class="row small"><span class="grow">«{j.name}» → {peerTitle(j.peer)} · {j.stage}</span><span class="muted">{j.done} / {j.total}</span></div><div class="bar"><i style="width:{j.total ? (j.done / j.total) * 100 : 3}%"></i></div></div>
  {/each}
  {#if sent.length}
    <div class="plbl" style="margin-top:12px">Отправленные</div>
    {#each sent as h}
      <div class="row small brow"><b>{h.name}</b><span class="muted">→ {peerTitle(h.peer)}</span>
        {#if h.error}<span style="color:var(--warn)">{h.error}</span>{:else}<span class="chip">{h.mode === 'clean' ? 'чистая' : 'патч'}</span><span class="muted">{h.files} {h.files === 1 ? 'файл' : 'файлов'} · {size(h.bytes)} → архив {size(h.zip)}</span>{/if}
        <div class="grow"></div><span class="muted" title={fullTime(h.at)}>{ago(h.at)}</span></div>
    {/each}
  {/if}
  {#if B && B.received.length}
    <div class="plbl" style="margin-top:12px">Принятые</div>
    {#each B.received as r}
      <div class="brow"><div class="row small"><b>{r.name}</b><span class="muted">от {peerTitle(r.peer)}</span>
        {#if r.error}<span style="color:var(--warn)">{r.error}</span>{:else}<span class="chip">{r.mode === 'clean' ? 'чистая' : 'патч'}</span><span class="muted">{r.files} файлов · {size(r.bytes)}</span>{/if}
        <div class="grow"></div><span class="muted" title={fullTime(r.at)}>{ago(r.at)}</span>
        <button class="b sm" onclick={() => openDir(r)}><Gi n="folder" /> Открыть</button><button class="b sm" onclick={() => setTarget(r)}>Куда…</button></div>
        <div class="small muted mono">{r.target}</div></div>
    {/each}
  {/if}
</div>

<style>
  .bgrid { display: grid; grid-template-columns: 1fr 2fr 1fr; gap: 10px; }
  @media (max-width: 900px) { .bgrid { grid-template-columns: 1fr; } }
  .brow { padding: 6px 0; border-top: 1px solid var(--line); gap: 8px; }
</style>
