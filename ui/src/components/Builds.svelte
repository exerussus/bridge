<script lang="ts">
  // Своя сборка (3.3.5 → 3.21): доставка каждому получателю по этапам, настройки раскрывающимися блоками,
  // отправка новой версии внизу. Номер ведёт отправитель: растёт, только когда меняются файлы (3.21).
  // Патч — только изменённое с прошлой отправки этому соседу; целиком — прежняя папка у соседа уходит в корзину.
  import { ui, toast, peerTitle, titleOf } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, size, fullTime } from '../lib/util';
  import Stepper from './Stepper.svelte';
  import AppVer from './AppVer.svelte';
  import { builtLocal, copyHash } from '../lib/appver';
  import { saving, saved as wasSaved, touched, markDeleted, eqIc } from '../lib/buildsync';

  type Preset = { id: string; name: string; path: string; peers: string[]; mode: 'patch' | 'clean'; ignore: string[]; exe: string; gid?: string; owner?: string; authors?: string[] };
  let { B, cur, onpick, reload, list = $bindable() }: { B: any; cur: string; onpick: (id: string) => void; reload: () => any; list: Preset[] } = $props();
  const peers = $derived(ui.ov.peers.filter((p: any) => p.enabled && !p.invite));
  let saveT: any = 0, saved = $state(''), busy = $state(false);
  let note = $state('');
  let pv = $state<any>(null), pvBusy = $state(false), pvErr = $state(''), pvKey = '';
  let pvT: any = 0;

  const guess = (p: string) => (p.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || '').replace(/[^\p{L}\p{N} ._-]/gu, '_');
  const newId = () => 'b' + Date.now().toString(36) + Math.random().toString(36).slice(2, 5);
  const P = $derived(list.find(p => p.id === cur) || null);
  const S = $derived(B ? (B.presets || []).find((p: any) => p.id === cur) : null); // как видит мост: доставка, версия, запуск
  const ignText = $derived(P ? P.ignore.join('\n') : '');

  function save(now = false) {
    clearTimeout(saveT); saved = 'сохраняю…';
    const go = async () => {
      const snap = saving();
      try { await api('POST', 'builds/presets', { list: $state.snapshot(list) }); wasSaved(snap); saved = 'сохранено'; reload(); }
      catch (e: any) { saved = ''; toast(e.message, 'bad'); }
    };
    if (now) go(); else saveT = setTimeout(go, 600);
  }
  function edit(f: (p: Preset) => void, refresh = false) { if (!P) return; f(P); touched(P.id); save(); if (refresh) previewSoon(); }
  function dup() {
    // копия — отдельная сборка: свой GUID, автор — этот ПК, без соавторов
    if (!P) return; const p: Preset = { ...$state.snapshot(P), id: newId(), name: (P.name + ' копия').slice(0, 60), gid: undefined, owner: undefined, authors: [] };
    list.push(p); touched(p.id); onpick(p.id); save(true);
  }
  function del() {
    if (!P || !confirm('Удалить заготовку «' + P.name + '»? У соседей уже присланное останется.')) return;
    const id = P.id; markDeleted(id); list = list.filter(x => x.id !== id); onpick(''); pv = null; save(true);
  }
  function toggleAuthor(n: string) { edit(p => { const a = p.authors || []; p.authors = a.some(x => eqIc(x, n)) ? a.filter(x => !eqIc(x, n)) : [...a, n]; }); }
  async function copyGid() { try { await navigator.clipboard.writeText(P?.gid || ''); toast('GUID скопирован', 'ok'); } catch { toast(P?.gid || '', 'ok'); } }
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
  $effect(() => { if (cur) { pv = null; pvKey = ''; previewSoon(0); } });

  async function flush() { clearTimeout(saveT); const snap = saving(); await api('POST', 'builds/presets', { list: $state.snapshot(list) }); wasSaved(snap); saved = 'сохранено'; }
  async function send(only = '') {
    if (!P) return;
    if (!P.path.trim()) { toast('Укажите папку сборки — полный путь', 'bad'); return; }
    if (!P.peers.length) { toast('Отметьте, кому отправлять', 'bad'); return; }
    if (!P.name.trim()) edit(p => { p.name = guess(p.path) || 'Сборка'; });
    busy = true;
    try {
      await flush();
      const r = await api('POST', 'builds/send', { preset: P.id, mode: P.mode, note, peer: only });
      note = '';
      toast('Упаковываю «' + P.name + '» для ' + r.started.map(titleOf).join(', ') + '…');
      if (r.errors?.length) toast(r.errors.join('; '), 'bad');
      reload();
    } catch (e: any) { toast(e.message, 'bad'); }
    busy = false;
  }
  async function now(peer: string) {
    try { const r = await api('POST', 'sync?peer=' + encodeURIComponent(peer)); toast(r.online ? titleOf(peer) + ' забирает сейчас' : titleOf(peer) + ': нет связи — ' + (r.error || 'не отвечает'), r.online ? 'ok' : 'bad'); reload(); }
    catch (e: any) { toast(e.message, 'bad'); }
  }
  async function nowAll() { for (const d of S?.delivery || []) if (d.state === 'waiting' && d.online) await now(d.peer); }
  async function runOwn() {
    try { const r = await api('POST', S?.running ? 'builds/stop' : 'builds/run', { preset: cur }); toast(S?.running ? 'Остановлено' : r.already ? 'Уже запущено' : 'Запущено у себя', 'ok'); reload(); } catch (e: any) { toast(e.message, 'bad'); }
  }

  // этапы доставки
  const STEPS = ['Упаковка', 'Ждёт соседа', 'Передача', 'Получено', 'Распаковано'];
  const AT: Record<string, number> = { packing: 0, waiting: 1, transfer: 2, received: 3, unpacked: 4, error: 4 };
  function detail(d: any): string {
    switch (d.state) {
      case 'packing': return d.detail || 'упаковываю…';
      case 'waiting': return d.online ? 'упаковано, ждёт, пока заберёт' : 'нет связи — отдам, как появится';
      case 'transfer': return 'забирает' + (d.speed ? ' · ' + size(d.speed) + '/с' : '') + (d.eta >= 0 ? ' · осталось ' + Math.max(1, d.eta) + ' с' : '');
      case 'received': return 'забрал — распаковывает (или его Мост старше 3.21 и не сообщает)';
      case 'unpacked': return 'распаковал' + (d.at ? ' · ' + ago(d.at) : '');
      case 'error': return 'не распаковал: ' + (d.error || 'ошибка');
      default: return 'ещё не отправляли';
    }
  }
  const sum = $derived.by(() => {
    const D = S?.delivery || [], c = (s: string) => D.filter((d: any) => d.state === s).length;
    const parts = [] as string[];
    if (D.length) parts.push('у ' + c('unpacked') + ' из ' + D.length);
    if (c('transfer')) parts.push(c('transfer') + ' передаётся');
    if (c('waiting')) parts.push(c('waiting') + ' ждёт');
    if (c('packing')) parts.push(c('packing') + ' упаковывается');
    if (c('error')) parts.push(c('error') + ' ошибка');
    return parts.join(' · ');
  });
  // (3.22) номер — самый большой из своих и тех, что стоят у получателей (общую сборку патчат и соавторы)
  const ownN = $derived(S?.latest?.n || 0);
  const ln = $derived(Math.max(ownN, S?.latestKnown || 0));
  const coLast = $derived(ln > ownN); // последнюю версию выпустил соавтор
  const isOwner = $derived(S ? !!S.mine : true);
  const authors = $derived(P?.authors || []);
  const me = $derived(ui.ov?.node?.node || '');
  // имя автора — node_name его ПК, у соседа может быть другой регистр; себя — «вы»
  const who = (n: string) => (eqIc(n, me) ? 'вы' : titleOf(peers.find((p: any) => eqIc(p.name, n))?.name || n));
  const isAuthor = (n: string) => authors.some(a => eqIc(a, n));
  // (3.22.1) флажки «Команды»: включённые соседи + все, кто уже в соавторах (выключенного/удалённого соседа можно снять)
  const team = $derived.by(() => {
    const T = peers.map((n: any) => ({ name: n.name, title: n.title || n.name, here: true }));
    const add = (n: string) => { if (n && !T.some(t => eqIc(t.name, n))) T.push({ name: n, title: who(n), here: eqIc(n, me) }); };
    if (!isOwner) add(P?.owner || '');
    for (const a of authors) add(a);
    return T;
  });
  // можно ли отправить этому получателю: у него ещё нет последней известной версии и сейчас ничего не идёт
  const canSend = (d: any) => d.state === 'error' || d.state === 'none' || (!['packing', 'transfer', 'received'].includes(d.state) && (d.has || 0) < ln);
  const mine = (h: any) => (h.preset ? h.preset === cur : P && h.name === P.name);
  const hist = $derived.by(() => {
    if (!B || !P) return [];
    // по версиям: номер → кому ушла, сколько файлов, заметка
    const by = new Map<number, any>();
    for (const h of B.history.filter((h: any) => h.dir === 'out' && mine(h) && !h.error && h.n)) {
      const v = by.get(h.n) || { n: h.n, at: h.at, note: h.note, files: 0, to: [] as string[], already: [] as string[], app: null as any };
      if (!v.app && h.app) v.app = h.app;
      if (h.already) v.already.push(h.peer); else { v.to.push(h.peer); v.files = Math.max(v.files, h.files); }
      if (h.at < v.at) v.at = h.at;
      if (!v.note && h.note) v.note = h.note;
      by.set(h.n, v);
    }
    return [...by.values()].sort((a, b) => b.n - a.n).slice(0, 12);
  });
  const errs = $derived(B && P ? B.history.filter((h: any) => h.dir === 'out' && mine(h) && h.error).slice(0, 3) : []);
  const UNITY = ['*_BurstDebugInformation_DoNotShip/', '*_BackUpThisFolder_ButDontShipItWithYourGame/'];
  const chev = '<path d="M6 9l6 6 6-6"/>';
</script>

{#if P}
  <section class="card bmine">
    <div class="row" style="gap:12px;flex-wrap:wrap">
      <div class="grow" style="min-width:240px">
        <h3 style="margin:0">Моя сборка</h3>
        <div class="bmt">{P.name || 'Без названия'} {#if ln}<span class="muted">№{ln}</span>{/if}</div>
        <div class="small ell bteam" class:muted={isOwner && !authors.length} title={coLast ? '№' + ln + ' выпустил соавтор; ваша последняя — №' + (ownN || '—') : ''}>{#if !isOwner}Вы соавтор — сборку ведёт {who(P.owner || '')}; номера общие, патч уходит от того, что лежит у получателя{:else if authors.length}Общая сборка — патчат и {authors.map(who).join(', ')}{:else}Патчите только вы — соавторов можно добавить в «Команде»{/if}{#if coLast} · №{ln} выпустил соавтор{/if}</div>
      </div>
      <span class="small muted svd">{saved}</span>
      <button class="b sm" onclick={dup}>Копия</button>
      <button class="b sm" disabled={!P.exe && !S?.running} title={P.exe ? P.exe + ' из папки сборки на этом ПК' : 'укажите, что запускать, в «Папке и запуске»'} onclick={runOwn}>{S?.running ? '■ Остановить у себя' : '▶ Запустить у себя'}</button>
      <button class="b sm danger" onclick={del}>Удалить</button>
    </div>

    {#if S?.app}
      {@const A = S.app}
      <!-- (3.23) version.json в папке сборки (AppCore): то, что уйдёт со следующей отправкой -->
      <div class="bapp">
        <div class="row" style="gap:12px;flex-wrap:wrap">
          <span class="bapl">Сборка Unity</span>
          <span class="bapv">v{A.version}</span>
          {#if A.fullVersion && A.fullVersion !== A.version}<span class="mono small">{A.fullVersion}</span>{/if}
          <div class="grow"></div>
          <span class="small muted">из version.json в папке сборки</span>
        </div>
        <div class="row bapc">
          {#if A.hash}<button class="b sm mono" title="Копировать хэш коммита" onclick={() => copyHash(A)}>коммит {A.hash.slice(0, 10)}</button>{/if}
          {#if A.dirty}<span class="pill warnf">+правки — не закоммичено</span>{/if}
          {#if A.profile}<span class="pill plain">{A.profile}</span>{/if}
          {#if A.platform}<span class="pill plain muted">{A.platform}</span>{/if}
          {#if A.development != null}<span class="pill plain">{A.development ? 'Development' : 'Release'}</span>{/if}
          {#if A.builtAtUtc}<span class="pill plain" title={A.builtAtUtc}>собрана {builtLocal(A)} · {ago(A.builtAtUtc)}</span>{/if}
        </div>
      </div>
    {/if}

    {#if (S?.delivery || []).length}
      <div class="fold2 bdel">
        <div class="row bdh"><h3 style="margin:0;width:150px">Доставка{ln ? ' №' + ln : ''}</h3><span class="muted small grow">{sum}</span>
          <button class="b sm" disabled={!(S?.delivery || []).some((d: any) => d.state === 'waiting' && d.online)} title="Попросить получателей забрать сборку прямо сейчас, не дожидаясь обмена" onclick={nowAll}>⟳ Отправить всем сейчас</button></div>
        {#each S.delivery as d (d.peer)}
          <div class="bdr">
            <div class="bdp">
              <div class="row" style="gap:8px"><b class="ell">{titleOf(d.peer)}</b><span class="pill plain {d.has && d.has === ln ? 'okf' : ''}">{d.has ? '№' + d.has : '—'}</span><span class="small"><AppVer a={d.app} mode="mini" /></span></div>
              <div class="small muted ell">{!d.has ? (d.sent ? 'получает №' + d.sent : 'ещё нет') : d.has === ln ? 'актуальная' : d.sent && d.sent > d.has ? 'догонит до №' + d.sent : 'отстаёт — «Отправить»'}</div>
            </div>
            <div class="bds">{#if d.state !== 'none'}<Stepper steps={STEPS} at={AT[d.state] ?? 0} pct={d.state === 'transfer' || d.state === 'packing' ? d.pct : null} bad={d.state === 'error'} />{/if}</div>
            <div class="bdd small ell" class:bad={d.state === 'error'} class:okc={d.state === 'unpacked'} title={detail(d)}>{detail(d)}</div>
            <div class="bda">
              {#if d.state === 'waiting'}<button class="b sm" disabled={!d.online} title={d.online ? 'Попросить забрать сейчас' : 'нет связи'} onclick={() => now(d.peer)}>Отправить сейчас</button>
              {:else}<button class="b sm" disabled={busy || !canSend(d)} title={canSend(d) ? 'Упаковать и отправить только ' + titleOf(d.peer) : (d.has || 0) >= ln ? 'У ' + titleOf(d.peer) + ' уже последняя версия №' + ln : 'идёт доставка'} onclick={() => send(d.peer)}>{d.state === 'error' ? 'Повторить' : 'Отправить'}</button>{/if}
            </div>
          </div>
        {/each}
      </div>
    {/if}

    <details class="fold2" open={!P.path}>
      <summary><h3 class="fh">Папка и запуск</h3><span class="mono muted small grow ell">{P.path || 'папка не указана'}{P.exe ? ' · ' + P.exe : ''}{S?.app ? ' · version.json v' + S.app.version : ''}</span><svg class="chev" viewBox="0 0 24 24" aria-hidden="true">{@html chev}</svg></summary>
      <div class="bgrid">
        <label class="f">Название<input value={P.name} placeholder={guess(P.path) || 'Моя игра'} oninput={e => edit(p => { p.name = (e.target as HTMLInputElement).value; })} /></label>
        <label class="f">Папка сборки на этом ПК<input class="mono" value={P.path} placeholder="D:\Проекты\Игра\Build" oninput={e => edit(p => { p.path = (e.target as HTMLInputElement).value; if (!p.name || p.name === 'Новая сборка') p.name = guess(p.path) || p.name; }, true)} /></label>
      </div>
      <label class="f">Что запускать — у себя и у получателей
        <input class="mono" list="bexes" value={P.exe} placeholder={pv?.exes?.[0] || 'Game.exe'} oninput={e => edit(p => { p.exe = (e.target as HTMLInputElement).value.trim(); })} />
        <datalist id="bexes">{#each pv?.exes || [] as x (x)}<option value={x}></option>{/each}</datalist>
      </label>
      {#if pv?.exes?.length && !P.exe}<div class="row small" style="gap:6px;flex-wrap:wrap;margin:-4px 0 12px"><span class="muted">Нашлось:</span>{#each pv.exes.slice(0, 4) as x (x)}<button class="b sm" onclick={() => edit(p => { p.exe = x; })}>{x}</button>{/each}</div>{/if}
    </details>

    <details class="fold2" open={!P.peers.length}>
      <summary><h3 class="fh">Кому</h3><span class="muted small grow ell">{P.peers.length ? P.peers.map(titleOf).join(', ') : 'никому — отметьте получателей'}</span><svg class="chev" viewBox="0 0 24 24" aria-hidden="true">{@html chev}</svg></summary>
      <div class="pchips">{#each peers as n (n.name)}<label class="pchip" class:on={P.peers.includes(n.name)}><input type="checkbox" checked={P.peers.includes(n.name)} onchange={() => togglePeer(n.name)} />{n.title || n.name}{#if !n.online}<span class="muted"> · нет связи</span>{/if}</label>{/each}</div>
    </details>

    <details class="fold2">
      <summary><h3 class="fh">Команда</h3><span class="muted small grow ell">{isOwner ? (authors.length ? 'патчат: вы и ' + authors.map(who).join(', ') : 'патчите только вы') : 'автор ' + who(P.owner || '') + ' · соавторы: ' + (authors.map(who).join(', ') || '—')}</span><svg class="chev" viewBox="0 0 24 24" aria-hidden="true">{@html chev}</svg></summary>
      <div class="row small" style="gap:10px;margin-bottom:10px"><span class="muted" style="width:110px">GUID сборки</span><span class="mono ell grow" title="У всех, кто патчит эту сборку, он один и тот же">{P.gid || 'выдаётся при сохранении…'}</span><button class="b sm" disabled={!P.gid} onclick={copyGid}>Копировать</button></div>
      <div class="row small" style="gap:10px;margin-bottom:8px"><span class="muted" style="width:110px">Автор</span><span>{isOwner ? 'вы' : who(P.owner || '')}</span></div>
      <div class="small muted" style="margin-bottom:8px">{isOwner ? 'Соавторы тоже могут выпускать версии этой сборки — у получателей она остаётся одной, номера идут подряд. Соавтор нажимает «Патчить эту сборку» на присланной ему сборке.' : 'Соавторов назначает автор. Он может убрать вас — тогда получатели перестанут принимать ваши патчи.'}</div>
      <div class="pchips">{#each team as n (n.name)}{@const own = !isOwner && eqIc(n.name, P.owner)}<label class="pchip" class:on={isAuthor(n.name) || own} class:ro={!isOwner || own} class:gone={!n.here} title={n.here ? '' : 'нет среди включённых соседей — можно снять с соавторов'}><input type="checkbox" disabled={!isOwner || own} checked={isAuthor(n.name) || own} onchange={() => toggleAuthor(n.name)} />{n.title}{#if own}<span class="muted"> · автор</span>{:else if !n.here}<span class="muted"> · не сосед</span>{/if}</label>{/each}</div>
    </details>

    <details class="fold2">
      <summary><h3 class="fh">Не паковать</h3><span class="muted small grow ell">{P.ignore.filter(l => l.trim() && !l.trim().startsWith('#')).length} правил{#if pv} · войдёт {pv.files} файлов, {size(pv.bytes)} · отсеется {size(pv.ignoredBytes)}{/if}</span><svg class="chev" viewBox="0 0 24 24" aria-hidden="true">{@html chev}</svg></summary>
      <textarea class="mono ign" rows="6" spellcheck="false" value={ignText} placeholder={'Library/\n*_BurstDebugInformation_DoNotShip/\n**/*.pdb\n!нужное.pdb'} oninput={e => edit(p => { p.ignore = (e.target as HTMLTextAreaElement).value.split('\n'); }, true)}></textarea>
      <div class="row small" style="gap:6px;flex-wrap:wrap;margin-top:6px">
        <button class="b sm" onclick={() => addIgn(UNITY)} title={UNITY.join('\n')}>+ Unity: отладка и бэкап</button>
        <button class="b sm" onclick={() => addIgn(['**/*.pdb'])}>+ *.pdb</button>
        <button class="b sm" onclick={() => addIgn(['*.log'])}>+ *.log</button>
        <span class="muted">По строке на шаблон: <code>имя</code> или <code>*.ext</code> — на любой глубине; <code>Папка/</code> — только папки; <code>!шаблон</code> — вернуть; <code>#</code> — комментарий.</span>
      </div>
      <div class="pv">
        {#if pvErr}<span style="color:var(--warn)">{pvErr}</span>
        {:else if pv}
          <div class="row" style="gap:14px;flex-wrap:wrap"><span>Войдёт: <b>{pv.files}</b> файлов · <b>{size(pv.bytes)}</b></span><span class="muted">отсеется: {pv.ignoredFiles} · {size(pv.ignoredBytes)}</span>{#if pvBusy}<span class="muted">считаю…</span>{/if}<div class="grow"></div><button class="b sm" onclick={() => { pvKey = ''; preview(); }}>Пересчитать</button></div>
          {#each pv.rules.slice(0, 8) as r (r.rule)}<div class="small row"><span class="chip ig">{r.rule}</span><span class="muted">{r.files} файлов · {size(r.bytes)}</span><span class="muted mono ell">например {r.example}</span></div>{/each}
          {#if pv.biggest.length}<details class="small"><summary class="muted">Самые большие из тех, что войдут</summary>{#each pv.biggest as b (b.path)}<div class="row"><span class="mono ell grow">{b.path}</span><span class="muted">{size(b.bytes)}</span></div>{/each}</details>{/if}
        {:else}<span class="muted">{pvBusy ? 'Считаю, что войдёт в архив…' : 'Укажите папку — покажу, что войдёт в архив и что отсеет игнор.'}</span>{/if}
      </div>
      <div style="height:14px"></div>
    </details>

    <details class="fold2">
      <summary><h3 class="fh">История</h3><span class="muted small grow ell">{hist.length ? hist.length + ' верс. · последняя ' + ago(hist[0].at) : 'ещё не отправляли'}</span><svg class="chev" viewBox="0 0 24 24" aria-hidden="true">{@html chev}</svg></summary>
      {#each hist as h (h.n)}
        <div class="brow"><div class="row small" style="gap:10px">
          <span class="pill plain">№{h.n}</span><span class="bhv">{#if h.app}<AppVer a={h.app} mode="mini" />{:else}<span class="muted">—</span>{/if}</span><span class="grow ell">{h.note || 'без заметки'}</span>
          <span class="muted ell">{h.to.length ? h.files + ' файл. · отдана ' + h.to.map(titleOf).join(', ') : ''}{h.already.length ? (h.to.length ? ' · ' : '') + 'уже была у ' + h.already.map(titleOf).join(', ') : ''}</span>
          <span class="muted" title={fullTime(h.at)} style="width:96px;text-align:right">{ago(h.at)}</span></div></div>
      {/each}
      {#each errs as h}<div class="small brow" style="color:var(--warn)">→ {titleOf(h.peer)}: {h.error} · {ago(h.at)}</div>{/each}
      <div style="height:10px"></div>
    </details>

    {#if ln}
      <div class="bhint small"><span class="dot" style="background:var(--cyan);box-shadow:0 0 8px var(--cyan)"></span><span>{#if coLast}Последнюю версию №{ln} выпустил соавтор. Если ваши файлы совпадают с ней, отправка уйдёт тем же №{ln}, иначе — №{ln + 1}.{:else}Номер растёт, только когда меняются файлы. Если с №{ln} ничего не менялось, отправка уйдёт тем же №{ln} тем, у кого его ещё нет.{/if}</span></div>
    {/if}
    <div class="row" style="gap:10px;flex-wrap:wrap">
      <input class="grow" style="min-width:260px" placeholder="Что нового в этой версии — увидят получатели" bind:value={note} maxlength="2000" aria-label="Что нового" />
      <span class="seg2"><button class:on={P.mode === 'patch'} title="Только изменённое с прошлой отправки (первая — всегда целиком)" onclick={() => edit(p => { p.mode = 'patch'; })}>Патч</button><button class:on={P.mode === 'clean'} title="Заменить у получателя целиком" onclick={() => edit(p => { p.mode = 'clean'; })}>Целиком</button></span>
      <button class="b pri" disabled={busy || !P.peers.length} onclick={() => send()}>Упаковать и отправить</button>
    </div>
  </section>
{/if}

<style>
  .bmine { display: flex; flex-direction: column; gap: 12px; margin-top: 16px; border-color: rgba(93, 224, 142, 0.25); }
  .bmt { font: 300 28px/1.2 var(--display); margin-top: 6px; }
  .fh { margin: 0 !important; width: 150px; flex: none; }
  .bdel { padding: 4px 16px 8px; }
  .bdh { min-height: 46px; gap: 10px; }
  .bdr { display: flex; gap: 16px; align-items: center; padding: 12px 0; border-top: 1px solid var(--line); flex-wrap: wrap; }
  .bdp { flex: 0 0 230px; min-width: 0; }
  .bds { flex: 1 1 360px; min-width: 280px; }
  .bdd { flex: 0 1 230px; min-width: 150px; color: var(--muted); }
  .bdd.bad { color: #ff7a8a; } .bdd.okc { color: var(--ok); }
  .bda { flex: 0 0 150px; display: flex; justify-content: flex-end; }
  .bgrid { display: grid; grid-template-columns: 1fr 2fr; gap: 10px; }
  @media (max-width: 900px) { .bgrid { grid-template-columns: 1fr; } }
  .pchips { display: flex; flex-wrap: wrap; gap: 6px; padding-bottom: 16px; }
  .pchip { display: inline-flex; align-items: center; gap: 6px; padding: 4px 12px; border-radius: 999px; border: 1px solid var(--line2); cursor: pointer; font-size: 13px; }
  .pchip.on { border-color: var(--ok); background: var(--ok-soft); }
  .pchip.ro { cursor: default; opacity: 0.8; }
  .pchip.gone { opacity: 0.6; border-style: dashed; }
  .bteam { margin-top: 4px; color: var(--cyan); min-height: 18px; }
  .bteam.muted { color: var(--muted); }
  .ign { width: 100%; box-sizing: border-box; resize: vertical; min-height: 96px; font-size: 13px; }
  .pv { margin-top: 10px; padding: 8px 10px; border-radius: 10px; background: var(--panel); min-height: 22px; display: flex; flex-direction: column; gap: 4px; }
  .brow { padding: 8px 0; border-top: 1px solid var(--line); }
  .bhint { display: flex; gap: 10px; align-items: center; padding: 10px 14px; border-radius: 12px; background: var(--accent-soft); border: 1px solid rgba(56, 225, 240, 0.18); }
  .svd { min-width: 80px; text-align: right; }
  code { font-size: 12px; }
  .bapp { display: flex; flex-direction: column; gap: 10px; padding: 12px 16px; border-radius: 14px; border: 1px solid rgba(56, 225, 240, 0.22); background: var(--accent-soft); }
  .bapl { width: 138px; font: 700 10.5px/1.4 var(--display); letter-spacing: .14em; text-transform: uppercase; color: var(--muted); }
  .bapv { font: 600 20px/1.2 var(--display); color: var(--cyan); }
  .bapc { gap: 8px; flex-wrap: wrap; padding-left: 150px; }
  @media (max-width: 900px) { .bapc { padding-left: 0; } }
  .bhv { width: 190px; flex: none; overflow: hidden; }
</style>
