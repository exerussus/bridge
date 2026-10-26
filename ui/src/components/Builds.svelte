<script lang="ts">
  // Сборки (3.3.5): заготовки — папка программы, кому, режим и игнор. Любую можно поправить и отправить новую версию.
  // Патч — только изменённое с прошлой отправки этой сборки этому соседу (с заменой); чистая — целиком,
  // прежняя папка у соседа уходит в корзину. Игнор — как .gitignore: что не нужно паковать (кэш, отладка).
  import { ui, toast, peerTitle, SET, setSet } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, size, fullTime } from '../lib/util';
  import Gi from './Gi.svelte';

  type Preset = { id: string; name: string; path: string; peers: string[]; mode: 'patch' | 'clean'; ignore: string[] };
  const peers = $derived(ui.ov.peers.filter((p: any) => p.enabled && !p.invite));
  let B = $state<any>(null);
  let list = $state<Preset[]>([]);
  let cur = $state(SET.buildCur || '');
  let loaded = false;
  let saveT: any = 0, saved = $state(''), busy = $state(false);
  let pv = $state<any>(null), pvBusy = $state(false), pvErr = $state(''), pvKey = '';
  let pvT: any = 0;

  const guess = (p: string) => (p.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || '').replace(/[^\p{L}\p{N} ._-]/gu, '_');
  const newId = () => 'b' + Date.now().toString(36) + Math.random().toString(36).slice(2, 5);
  const P = $derived(list.find(p => p.id === cur) || null);
  const ignText = $derived(P ? P.ignore.join('\n') : '');

  async function load() {
    try {
      B = await api('GET', 'builds');
      if (!loaded) {
        loaded = true;
        list = (B.presets || []).map((p: any) => ({ ...p, peers: p.peers || [], ignore: p.ignore || [] }));
        // прежняя одиночная сборка (3.3.1–3.3.4) — первой заготовкой
        if (!list.length && SET.buildSrc) { list = [{ id: newId(), name: SET.buildName || guess(SET.buildSrc) || 'Сборка', path: SET.buildSrc, peers: SET.buildPeer ? [SET.buildPeer] : [], mode: SET.buildMode === 'clean' ? 'clean' : 'patch', ignore: [] }]; save(); }
        if (!list.some(p => p.id === cur)) cur = list[0]?.id || '';
      }
    } catch {}
  }
  $effect(() => {
    let t: any = 0, live = true;
    const tick = async () => { await load(); if (live) t = setTimeout(tick, B && B.jobs.length ? 1000 : 5000); };
    tick();
    return () => { live = false; clearTimeout(t); };
  });
  $effect(() => { if (cur !== SET.buildCur) setSet('buildCur', cur); });

  function save(now = false) {
    clearTimeout(saveT); saved = '…';
    const go = async () => {
      try { const r = await api('POST', 'builds/presets', { list: $state.snapshot(list) }); saved = 'сохранено'; void r; }
      catch (e: any) { saved = ''; toast(e.message, 'bad'); }
    };
    if (now) go(); else saveT = setTimeout(go, 600);
  }
  function edit(f: (p: Preset) => void, refresh = false) { if (!P) return; f(P); save(); if (refresh) previewSoon(); }
  function add() {
    const p: Preset = { id: newId(), name: 'Новая сборка', path: '', peers: peers[0] ? [peers[0].name] : [], mode: 'patch', ignore: [] };
    list.push(p); cur = p.id; pv = null; save(true);
  }
  function dup() {
    if (!P) return; const p: Preset = { ...$state.snapshot(P), id: newId(), name: (P.name + ' копия').slice(0, 60) };
    list.push(p); cur = p.id; save(true); previewSoon();
  }
  function del() {
    if (!P || !confirm('Удалить заготовку «' + P.name + '»? У соседей уже присланное останется.')) return;
    list = list.filter(x => x.id !== P!.id); cur = list[0]?.id || ''; pv = null; save(true);
  }
  function pick(id: string) { cur = id; pv = null; pvErr = ''; previewSoon(0); }
  function togglePeer(n: string) { edit(p => { p.peers = p.peers.includes(n) ? p.peers.filter(x => x !== n) : [...p.peers, n]; }); }
  function addIgn(lines: string[]) { edit(p => { for (const l of lines) if (!p.ignore.includes(l)) p.ignore.push(l); }, true); }

  function previewSoon(ms = 700) { clearTimeout(pvT); pvT = setTimeout(preview, ms); }
  async function preview() {
    if (!P || !P.path.trim()) { pv = null; pvErr = ''; return; }
    const key = P.path + '\n' + P.ignore.join('\n'); if (key === pvKey && pv) return;
    pvBusy = true; pvErr = '';
    try { pv = await api('POST', 'builds/preview', { path: P.path.trim(), ignore: $state.snapshot(P.ignore) }); pvKey = key; }
    catch (e: any) { pv = null; pvErr = e.message; }
    pvBusy = false;
  }
  $effect(() => { if (cur && loaded) previewSoon(0); });

  async function send(mode: 'patch' | 'clean') {
    if (!P) return;
    if (!P.path.trim()) { toast('Укажите папку сборки — полный путь', 'bad'); return; }
    if (!P.peers.length) { toast('Отметьте, кому отправлять', 'bad'); return; }
    if (!P.name.trim()) edit(p => { p.name = guess(p.path) || 'Сборка'; });
    busy = true;
    try {
      clearTimeout(saveT); await api('POST', 'builds/presets', { list: $state.snapshot(list) }); saved = 'сохранено';
      const r = await api('POST', 'builds/send', { preset: P.id, mode });
      toast('Упаковываю «' + P.name + '» для ' + r.started.map(peerTitle).join(', ') + '…');
      if (r.errors?.length) toast(r.errors.join('; '), 'bad');
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

  const mine = (p: Preset, h: any) => h.preset ? h.preset === p.id : h.name === p.name;
  const lastOf = (p: Preset) => (B ? B.history.find((h: any) => h.dir === 'out' && mine(p, h)) : null);
  const jobsOf = (p: Preset) => (B ? B.jobs.filter((j: any) => mine(p, j)) : []);
  const hist = $derived(P && B ? B.history.filter((h: any) => h.dir === 'out' && mine(P, h)).slice(0, 8) : []);
  const UNITY = ['*_BurstDebugInformation_DoNotShip/', '*_BackUpThisFolder_ButDontShipItWithYourGame/'];
</script>

<div class="bwrap">
  <div class="card blist">
    <div class="row"><h3 style="margin:0">Сборки</h3><div class="grow"></div><button class="b sm pri" onclick={add}>+ Новая</button></div>
    <div class="small muted" style="margin:6px 0 8px">Заготовка — папка программы, кому и что не паковать. Новую версию — одной кнопкой.</div>
    {#each list as p (p.id)}
      {@const l = lastOf(p)}
      {@const js = jobsOf(p)}
      <button class="bitem" class:on={p.id === cur} onclick={() => pick(p.id)}>
        <span class="row"><b class="grow ell">{p.name || 'Без названия'}</b>{#if js.length}<span class="chip">упаковка…</span>{/if}</span>
        <span class="small muted mono ell">{p.path || 'папка не указана'}</span>
        <span class="small muted ell">→ {p.peers.length ? p.peers.map(peerTitle).join(', ') : 'никому'}{#if l} · {l.error ? 'ошибка' : 'отправлена ' + ago(l.at)}{/if}</span>
      </button>
    {:else}
      <div class="empty">Пока ни одной — «+ Новая»</div>
    {/each}
  </div>

  <div class="card bed">
    {#if P}
      <div class="row"><h3 style="margin:0" class="grow ell">{P.name || 'Без названия'}</h3><span class="small muted svd">{saved}</span>
        <button class="b sm" onclick={dup}>Копия</button><button class="b sm danger" onclick={del}>Удалить</button></div>
      <div class="bgrid">
        <label class="f">Название<input value={P.name} placeholder={guess(P.path) || 'CORA'} oninput={e => edit(p => { p.name = (e.target as HTMLInputElement).value; })} /></label>
        <label class="f">Папка сборки на этом ПК<input class="mono" value={P.path} placeholder="D:\Unity\CORA\Build" oninput={e => edit(p => { p.path = (e.target as HTMLInputElement).value; if (!p.name || p.name === 'Новая сборка') p.name = guess(p.path) || p.name; }, true)} /></label>
      </div>
      <div class="f" style="margin-top:10px">Кому
        <div class="pchips">{#each peers as n (n.name)}<label class="pchip" class:on={P.peers.includes(n.name)}><input type="checkbox" checked={P.peers.includes(n.name)} onchange={() => togglePeer(n.name)} />{n.title || n.name}{#if !n.online}<span class="muted"> · нет связи</span>{/if}</label>{/each}</div>
      </div>
      <div class="f" style="margin-top:10px">Не паковать (игнор)
        <textarea class="mono ign" rows="6" spellcheck="false" value={ignText} placeholder={'Library/\n*_BurstDebugInformation_DoNotShip/\n**/*.pdb\n!нужное.pdb'} oninput={e => edit(p => { p.ignore = (e.target as HTMLTextAreaElement).value.split('\n'); }, true)}></textarea>
      </div>
      <div class="row small" style="gap:6px;flex-wrap:wrap;margin-top:4px">
        <button class="b sm" onclick={() => addIgn(UNITY)} title={UNITY.join('\n')}>+ Unity: отладка и бэкап</button>
        <button class="b sm" onclick={() => addIgn(['**/*.pdb'])}>+ *.pdb</button>
        <button class="b sm" onclick={() => addIgn(['*.log'])}>+ *.log</button>
        <span class="muted">По строке на шаблон: <code>имя</code> или <code>*.ext</code> — на любой глубине; <code>Папка/</code> — только папки; <code>путь/от/корня</code>, <code>**/</code> — любая глубина; <code>!шаблон</code> — вернуть; <code>#</code> — комментарий.</span>
      </div>

      <div class="pv">
        {#if pvErr}<span style="color:var(--warn)">{pvErr}</span>
        {:else if pv}
          <div class="row" style="gap:14px;flex-wrap:wrap"><span>Войдёт: <b>{pv.files}</b> файлов · <b>{size(pv.bytes)}</b></span><span class="muted">отсеется: {pv.ignoredFiles} · {size(pv.ignoredBytes)}</span>{#if pvBusy}<span class="muted">считаю…</span>{/if}<div class="grow"></div><button class="b sm" onclick={() => { pvKey = ''; preview(); }}>Пересчитать</button></div>
          {#each pv.rules.slice(0, 8) as r (r.rule)}<div class="small row"><span class="chip ig">{r.rule}</span><span class="muted">{r.files} файлов · {size(r.bytes)}</span><span class="muted mono ell">например {r.example}</span></div>{/each}
          {#if pv.biggest.length}<details class="small"><summary class="muted">Самые большие из тех, что войдут</summary>{#each pv.biggest as b (b.path)}<div class="row"><span class="mono ell grow">{b.path}</span><span class="muted">{size(b.bytes)}</span></div>{/each}</details>{/if}
        {:else}<span class="muted">{pvBusy ? 'Считаю, что войдёт в архив…' : 'Укажите папку — покажу, что войдёт в архив и что отсеет игнор.'}</span>{/if}
      </div>

      <div class="row" style="gap:10px;flex-wrap:wrap;margin-top:12px">
        <span class="seg"><button class:on={P.mode === 'patch'} onclick={() => edit(p => { p.mode = 'patch'; })}>Патч — только изменённое</button><button class:on={P.mode === 'clean'} onclick={() => edit(p => { p.mode = 'clean'; })}>Чистая — заменить целиком</button></span>
        <button class="b pri" disabled={busy || !P.peers.length} onclick={() => send(P!.mode)}>Упаковать и отправить</button>
        <span class="small muted">Первая отправка соседу — всегда целиком.</span>
      </div>
      {#each jobsOf(P) as j}
        <div style="margin-top:8px"><div class="row small"><span class="grow">→ {peerTitle(j.peer)} · {j.stage}</span><span class="muted">{j.done} / {j.total}</span></div><div class="bar"><i style="width:{j.total ? (j.done / j.total) * 100 : 3}%"></i></div></div>
      {/each}
      {#if hist.length}
        <div class="plbl" style="margin-top:12px">Отправки</div>
        {#each hist as h}
          <div class="row small brow"><span class="muted">→ {peerTitle(h.peer)}</span>
            {#if h.error}<span style="color:var(--warn)">{h.error}</span>{:else}<span class="chip">{h.mode === 'clean' ? 'чистая' : 'патч'}</span><span class="muted">{h.files} {h.files === 1 ? 'файл' : 'файлов'} · {size(h.bytes)} → архив {size(h.zip)}{h.skipped ? ' · отсеяно ' + h.skipped : ''}</span>{/if}
            <div class="grow"></div><span class="muted" title={fullTime(h.at)}>{ago(h.at)}</span></div>
        {/each}
      {/if}
    {:else}
      <div class="empty">Выберите сборку слева или создайте новую</div>
    {/if}
  </div>
</div>

{#if B && B.received.length}
  <div class="card" style="margin-top:14px"><h3 style="margin:0 0 6px">Принятые сборки</h3>
    {#each B.received as r}
      <div class="brow"><div class="row small"><b>{r.name}</b><span class="muted">от {peerTitle(r.peer)}</span>
        {#if r.error}<span style="color:var(--warn)">{r.error}</span>{:else}<span class="chip">{r.mode === 'clean' ? 'чистая' : 'патч'}</span><span class="muted">{r.files} файлов · {size(r.bytes)}</span>{/if}
        <div class="grow"></div><span class="muted" title={fullTime(r.at)}>{ago(r.at)}</span>
        <button class="b sm" onclick={() => openDir(r)}><Gi n="folder" /> Открыть</button><button class="b sm" onclick={() => setTarget(r)}>Куда…</button></div>
        <div class="small muted mono">{r.target}</div></div>
    {/each}
  </div>
{/if}

<style>
  .bwrap { display: grid; grid-template-columns: minmax(220px, 300px) 1fr; gap: 14px; align-items: start; }
  @media (max-width: 900px) { .bwrap { grid-template-columns: 1fr; } }
  .bitem { display: flex; flex-direction: column; gap: 2px; width: 100%; text-align: left; padding: 8px 10px; margin-top: 6px; border-radius: 10px; border: 1px solid var(--line); background: var(--panel2); color: inherit; cursor: pointer; font: inherit; min-width: 0; }
  .bitem.on { border-color: var(--acc, #38E1F0); box-shadow: 0 0 0 1px var(--acc, #38E1F0) inset; }
  .ell { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; max-width: 100%; }
  .bgrid { display: grid; grid-template-columns: 1fr 2fr; gap: 10px; margin-top: 10px; }
  @media (max-width: 900px) { .bgrid { grid-template-columns: 1fr; } }
  .pchips { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }
  .pchip { display: inline-flex; align-items: center; gap: 6px; padding: 3px 10px; border-radius: 14px; border: 1px solid var(--line); background: var(--panel2); cursor: pointer; font-size: 13px; }
  .pchip.on { border-color: var(--acc, #38E1F0); }
  .ign { width: 100%; box-sizing: border-box; resize: vertical; min-height: 96px; font-size: 13px; }
  .pv { margin-top: 10px; padding: 8px 10px; border-radius: 10px; background: var(--panel2); min-height: 22px; display: flex; flex-direction: column; gap: 4px; }
  .brow { padding: 6px 0; border-top: 1px solid var(--line); gap: 8px; }
  .svd { min-width: 70px; text-align: right; }
  code { font-size: 12px; }
</style>
