<script lang="ts">
  // «Сборки» (3.21): свои и присланные — одной сеткой карточек. На каждой — «▶ Запустить» (и у своих: из её папки),
  // у своих — «Отправить» или «⟳ Сейчас», если версия ждёт, пока сосед заберёт. Ниже — выбранная сборка подробно.
  import AppVer from '../components/AppVer.svelte';
  import { SET, setSet, toast, titleOf } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, size } from '../lib/util';
  import Builds from '../components/Builds.svelte';
  import ReceivedBuilds from '../components/ReceivedBuilds.svelte';
  import Gi from '../components/Gi.svelte';
  import { BS, saving, saved as wasSaved, isDirty, recentlyDeleted, runKey } from '../lib/buildsync';

  type Preset = { id: string; name: string; path: string; peers: string[]; mode: 'patch' | 'clean'; ignore: string[]; exe: string; gid?: string; owner?: string; authors?: string[] };
  let B = $state<any>(null);
  let list = $state<Preset[]>([]);
  let loaded = false;
  let tab = $state<string>(SET.buildsTab === 'in' || SET.buildsTab === 'mine' ? SET.buildsTab : 'all');
  let sel = $state<string>(SET.buildSel || '');
  let busy = $state('');

  async function load() {
    try {
      // (3.22.1) ответ, начатый до последнего сохранения заготовок, устарел — не подмешивать (воскрешал удалённые)
      const g = BS.gen;
      const x = await api('GET', 'builds');
      if (g !== BS.gen) return;
      x.presets = (x.presets || []).filter((p: any) => !recentlyDeleted(p.id));
      B = x;
      if (!loaded) {
        loaded = true;
        list = (B.presets || []).map((p: any) => ({ id: p.id, name: p.name, path: p.path, peers: p.peers || [], mode: p.mode === 'clean' ? 'clean' : 'patch', ignore: p.ignore || [], exe: p.exe || '', gid: p.gid, owner: p.owner, authors: p.authors || [] }));
        const ok = sel.startsWith('p:') ? list.some(p => 'p:' + p.id === sel) : sel.startsWith('r:') ? B.received.some((r: any) => 'r:' + r.peer + '/' + r.name === sel) : false;
        if (!ok) sel = list[0] ? 'p:' + list[0].id : B.received[0] ? 'r:' + B.received[0].peer + '/' + B.received[0].name : '';
      } else {
        // (3.22) новые заготовки получают GUID и автора на сервере; подхваченная («Патчить эту сборку») — целиком
        for (const s of B.presets || []) {
          if (recentlyDeleted(s.id)) continue;
          const p = list.find(x => x.id === s.id);
          if (!p) list.push({ id: s.id, name: s.name, path: s.path, peers: s.peers || [], mode: s.mode === 'clean' ? 'clean' : 'patch', ignore: s.ignore || [], exe: s.exe || '', gid: s.gid, owner: s.owner, authors: s.authors || [] });
          else if (!isDirty(p.id)) {
            // команда могла поменяться с другого окна или телефона — без несохранённых правок берём, как у моста
            if (p.gid !== s.gid) p.gid = s.gid;
            if (p.owner !== s.owner) p.owner = s.owner;
            const a = s.authors || [];
            if (JSON.stringify(p.authors || []) !== JSON.stringify(a)) p.authors = a;
          }
        }
      }
    } catch {}
  }
  $effect(() => {
    let t: any = 0, live = true;
    const busyNow = () => B && (B.jobs.length || B.received.some((r: any) => r.running) || (B.presets || []).some((p: any) => p.running || (p.delivery || []).some((d: any) => ['packing', 'transfer', 'waiting', 'received'].includes(d.state))));
    const tick = async () => { await load(); if (live) t = setTimeout(tick, busyNow() ? 1500 : 5000); };
    tick();
    return () => { live = false; clearTimeout(t); };
  });
  const cur = $derived(sel.startsWith('p:') && list.some(p => 'p:' + p.id === sel) ? sel.slice(2) : '');
  function pick(s: string) { sel = s; setSet('buildSel', s); }
  function setTab(t: string) { tab = t; setSet('buildsTab', t); }
  function add() {
    const p: Preset = { id: 'b' + Date.now().toString(36) + Math.random().toString(36).slice(2, 5), name: 'Новая сборка', path: '', peers: [], mode: 'patch', ignore: [], exe: '' };
    list.push(p);
    const snap = saving();
    api('POST', 'builds/presets', { list: $state.snapshot(list) }).then(() => { wasSaved(snap); return load(); }).catch(e => toast(e.message, 'bad'));
    pick('p:' + p.id); if (tab === 'in') setTab('all');
  }

  type Card = { key: string; mine: boolean; name: string; who: string; ver: string; note: string; meta: string; running: boolean; canRun: boolean; runTitle: string; waiting: string[]; app: any; p?: any; r?: any };
  const cards = $derived.by(() => {
    if (!B) return [] as Card[];
    const C: Card[] = [];
    if (tab !== 'in') for (const p of B.presets || []) {
      const D = p.delivery || [], n = Math.max(p.latest?.n || 0, p.latestKnown || 0) || undefined;
      const c = (s: string) => D.filter((d: any) => d.state === s).length;
      const job = D.find((d: any) => d.state === 'packing');
      const parts = [] as string[];
      if (D.length && n) parts.push('у ' + c('unpacked') + ' из ' + D.length);
      if (c('transfer')) parts.push('передаётся ' + c('transfer'));
      if (c('waiting')) parts.push('ждёт ' + c('waiting'));
      if (c('error')) parts.push('ошибка ' + c('error'));
      const own = p.latest?.n || 0, kn = p.latestKnown || 0;
      const team = p.mine ? (p.authors?.length ? 'моя, с соавторами' : 'моя') : 'соавтор · автор ' + titleOf(p.owner);
      C.push({ key: 'p:' + p.id, mine: true, name: p.name || 'Без названия', who: team + (p.peers?.length ? ' → ' + p.peers.map(titleOf).join(', ') : ' · получатели не выбраны'), ver: n ? '№' + n : '—',
        note: kn > own ? '№' + kn + ' выпустил соавтор' + (own ? ' · ваша последняя — №' + own : '') : p.latest?.note || (p.path ? '' : 'Укажите папку сборки'), meta: job ? 'упаковка ' + (job.pct ?? 0) + '%' : (n ? '№' + n + ': ' : '') + (parts.join(' · ') || (p.path ? 'ещё не отправляли' : '')),
        running: !!p.running, canRun: !!p.exe, runTitle: p.exe ? p.exe + ' из ' + p.path : 'укажите, что запускать', waiting: D.filter((d: any) => d.state === 'waiting' && d.online).map((d: any) => d.peer),
        // (3.23) версия программы: у последней отправленной; не отправляли — что лежит в папке
        app: p.latest ? p.latest.app || null : p.app || null, p });
    }
    if (tab !== 'mine') for (const r of B.received) {
      const L = r.lastOk || r;
      const from = r.gid && r.by && r.by !== r.peer ? titleOf(r.by) + ' (автор ' + titleOf(r.peer) + ')' : titleOf(r.peer);
      C.push({ key: 'r:' + r.peer + '/' + r.name, mine: false, name: r.name, who: 'от ' + from + ' · ' + ago(L.at), ver: L.n ? '№' + L.n : '—', note: L.note || '',
        meta: r.error ? 'последняя не распаковалась' : (L.mode === 'clean' ? 'целиком' : 'патч') + ' · ' + L.files + ' файл. · ' + size(L.bytes || 0) + (r.exe ? ' · ' + r.exe : ''),
        running: !!r.running, canRun: !!r.exe && !!r.exeExists, runTitle: r.exeExists ? r.exe : r.exe ? 'нет файла ' + r.exe + ' в папке сборки' : 'не указано, что запускать', waiting: [], app: L.app || null, r });
    }
    return C;
  });
  const nMine = $derived((B?.presets || []).length), nIn = $derived(B?.received.length || 0);
  const selR = $derived(B && sel.startsWith('r:') ? B.received.find((r: any) => 'r:' + r.peer + '/' + r.name === sel) : null);

  async function run(c: Card) {
    busy = c.key;
    try {
      if (c.mine) { const x = await api('POST', c.running ? 'builds/stop' : 'builds/run', { preset: c.p.id }); toast(c.running ? 'Остановлено' : x.already ? 'Уже запущено' : 'Запущено у себя', 'ok'); }
      else if (c.running) { await api('POST', 'builds/stop', { peer: c.r.peer, name: c.r.name }); toast('Остановлено'); }
      else {
        const k = runKey(c.r), okl = (SET.buildRunOk || []) as string[];
        if (!okl.includes(k)) {
          if (!confirm('Запустить «' + c.r.exe + '» из сборки «' + c.r.name + '» от ' + titleOf(c.r.peer) + (c.r.gid && c.r.by && c.r.by !== c.r.peer ? ' (эту версию выпустил ' + titleOf(c.r.by) + ')' : '') + '?\n\nЭто программа с другого ПК: она получит те же возможности, что любая программа на вашем компьютере. Запускайте, если доверяете отправителю.')) { busy = ''; return; }
          setSet('buildRunOk', [...okl, k].slice(-100));
        }
        const x = await api('POST', 'builds/run', { peer: c.r.peer, name: c.r.name }); toast(x.already ? 'Уже запущено' : 'Запущено: ' + c.r.exe, 'ok');
      }
      load();
    } catch (e: any) { toast(e.message, 'bad'); }
    busy = '';
  }
  async function sendCard(c: Card) {
    busy = c.key;
    try {
      if (c.waiting.length) { for (const p of c.waiting) await api('POST', 'sync?peer=' + encodeURIComponent(p)); toast('Попросил забрать: ' + c.waiting.map(titleOf).join(', '), 'ok'); }
      else { pick(c.key); const r = await api('POST', 'builds/send', { preset: c.p.id }); toast('Упаковываю «' + c.name + '» для ' + r.started.map(titleOf).join(', ') + '…'); if (r.errors?.length) toast(r.errors.join('; '), 'bad'); }
      load();
    } catch (e: any) { toast(e.message, 'bad'); }
    busy = '';
  }
</script>

<div class="row" style="gap:10px;flex-wrap:wrap;margin:-6px 0 16px">
  <div class="seg2" role="tablist" aria-label="Чьи сборки">
    <button role="tab" class:on={tab === 'all'} onclick={() => setTab('all')}>Все<span class="cnt2">{nMine + nIn}</span></button>
    <button role="tab" class:on={tab === 'in'} onclick={() => setTab('in')}>Присланные<span class="cnt2">{nIn}</span></button>
    <button role="tab" class:on={tab === 'mine'} onclick={() => setTab('mine')}>Мои<span class="cnt2">{nMine}</span></button>
  </div>
  <div class="grow"></div>
  <button class="b pri" onclick={add}>＋ Новая сборка</button>
</div>

{#if !B}<div class="empty">Загружаю…</div>
{:else if !cards.length}
  <div class="card"><div class="empty">{tab === 'in' ? 'Вам пока ничего не присылали. Когда сосед отправит сборку, она появится здесь: версия, что нового и кнопка запуска.' : 'Пока ни одной сборки — «＋ Новая сборка»: папка программы, кому и что не паковать.'}</div></div>
{:else}
  <div class="bcards">
    {#each cards as c (c.key)}
      <div class="card bc" class:sel={sel === c.key}>
        <button class="bch" onclick={() => pick(c.key)} aria-label={'Подробнее: ' + c.name}>
          <span class="bico" class:mine={c.mine}><Gi n="box" /></span>
          <span class="grow" style="min-width:0"><span class="bcn ell">{c.name}</span><span class="small muted ell" style="display:block">{c.who}</span></span>
          <span class="pill plain">{c.ver}</span>
        </button>
        <AppVer a={c.app} />
        <div class="bcnote">{c.note}</div>
        <div class="small muted ell" title={c.meta}>{c.meta}</div>
        <div class="row bcact">
          {#if c.running}
            <span class="grow row" style="gap:8px;color:var(--ok);font-weight:600"><span class="dot ok"></span>запущено</span>
            <button class="b sm danger" disabled={busy === c.key} onclick={() => run(c)}>Остановить</button>
          {:else}
            <button class="b pri grow" disabled={busy === c.key || !c.canRun} title={c.runTitle} onclick={() => run(c)}>▶ Запустить</button>
          {/if}
          {#if c.mine}<button class="b sm" disabled={busy === c.key} title={c.waiting.length ? 'Попросить ' + c.waiting.map(titleOf).join(', ') + ' забрать сейчас' : 'Упаковать и отправить новую версию всем получателям'} onclick={() => sendCard(c)}>{c.waiting.length ? '⟳ Сейчас' : '↑ Отправить'}</button>{/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

{#if B && cur}
  <Builds {B} {cur} onpick={id => pick(id ? 'p:' + id : list[0] ? 'p:' + list[0].id : '')} bind:list reload={load} />
{:else if B && selR}
  <div style="margin-top:16px"><ReceivedBuilds B={{ ...B, received: [selR] }} reload={load} onpreset={(id: string) => pick('p:' + id)} /></div>
{/if}

<style>
  .bcards { display: grid; grid-template-columns: repeat(auto-fill, minmax(290px, 1fr)); gap: 14px; }
  .bc { display: flex; flex-direction: column; gap: 10px; padding: 18px; }
  .bc.sel { border-color: rgba(93, 224, 142, 0.55); box-shadow: 0 0 0 1px rgba(93, 224, 142, 0.35) inset; }
  .bch { display: flex; gap: 14px; align-items: center; border: 0; background: none; color: inherit; font: inherit; text-align: left; padding: 0; cursor: pointer; min-width: 0; }
  .bico { width: 46px; height: 46px; border-radius: 14px; display: grid; place-items: center; flex: none; background: var(--accent-soft); color: var(--cyan); }
  .bico.mine { background: var(--ok-soft); color: var(--ok); }
  .bcn { display: block; font: 600 17px/1.25 var(--display); }
  .bcnote { font-size: 13px; color: var(--text); opacity: 0.85; min-height: 38px; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
  .bcact { gap: 8px; min-height: 36px; flex-wrap: nowrap; }
</style>
