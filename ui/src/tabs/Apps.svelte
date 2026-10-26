<script lang="ts">
  // Раздел «Приложения»: рабочий стол ярлыков (поиск, перестановка, меню по правой кнопке) и открытые приложения
  // вкладками. Веб-интерфейс — изолированная рамка; своё окно (Unity и т. п.) — панель управления; кнопки — форма.
  import { api } from '../lib/api';
  import { toast, openModal, SET, ui, titleOf } from '../lib/state.svelte';
  import { AP, byId, ordered, reorder, openApp, closeApp, reloadApp, popout, frameSrc, sandboxOf, allowOf, onFrameMessage, emitTo, startApps, closeNet, loadSessions, importPackages, loadApps, PERM_LABEL, type AppInfo } from '../lib/apps.svelte';
  import AppConsent from '../modals/AppConsent.svelte';
  import Gi from '../components/Gi.svelte';

  startApps();
  let q = $state('');
  let menu = $state<{ id: string; x: number; y: number } | null>(null);
  let drag = $state('');
  let over = $state('');
  let logOf = $state<{ id: string; lines: string[] } | null>(null);
  let actOut = $state<Record<string, { busy: boolean; out: string; ok: boolean }>>({});
  let params = $state<Record<string, Record<string, string>>>({});
  let dropHover = $state(false);
  // что идёт сейчас в моём канале (сеансы приложений — свои и соседей)
  let live = $state<any>(null);
  $effect(() => {
    let t: any = 0, on = true;
    const tick = async () => { if (ui.tab === 'apps') live = await loadSessions(); if (on) t = setTimeout(tick, 3000); };
    tick();
    return () => { on = false; clearTimeout(t); };
  });
  const liveList = $derived(((live?.sessions || []) as any[]).filter(s => s.room === live?.channel?.room));
  function joinLive(s: any) {
    const a = byId(s.app);
    if (!a) { toast('У вас нет приложения «' + s.app + '» — попросите хозяина поделиться им', 'bad'); return; }
    open(a);
  }

  const list = $derived(ordered().filter(a => !q.trim() || (a.name + ' ' + a.description).toLowerCase().includes(q.trim().toLowerCase())));
  const act = $derived(AP.active ? byId(AP.active) : undefined);
  const uiOpen = $derived(AP.open.map(id => byId(id)).filter((a): a is AppInfo => !!a && a.ui && a.state === 'ok'));

  const hue = (id: string) => { let h = 0; for (const c of id) h = (h * 31 + c.charCodeAt(0)) >>> 0; return h % 360; };
  function badge(a: AppInfo): [string, string] {
    if (a.state === 'unsupported') return ['не для этой системы', 'quiet'];
    if (a.state === 'new') return ['новое · разрешить', 'warn'];
    if (a.state === 'changed') return ['обновлено · разрешить', 'warn'];
    if (a.runningNative) return ['запущено', 'good'];
    if (a.runningService) return ['служба работает', 'good'];
    if (a.native) return ['своё окно', 'quiet'];
    if (!a.ui && !a.actions.length && a.net) return ['сетевое', 'quiet'];
    if (a.ui && a.service) return ['со службой', 'quiet'];
    if (a.ui) return ['веб', 'quiet'];
    return ['кнопки', 'quiet'];
  }
  function open(a: AppInfo) {
    menu = null;
    if (a.state === 'unsupported') { toast('«' + a.name + '» не для этой системы (' + a.unsupported + ')', 'bad'); return; }
    if (a.state !== 'ok') { openModal(AppConsent, { id: a.id, then: true }); return; }
    openApp(a.id);
  }
  function ctx(e: MouseEvent, a: AppInfo) { e.preventDefault(); menu = { id: a.id, x: e.clientX, y: e.clientY }; }
  async function call(path: string, body: any, ok?: string) {
    menu = null;
    try { await api('POST', path, body); if (ok) toast(ok, 'ok'); await loadApps(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function showLog(id: string) {
    menu = null;
    try { const r = await api('GET', 'apps/log?id=' + encodeURIComponent(id)); logOf = { id, lines: r.lines }; } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function remove(a: AppInfo) {
    menu = null;
    if (!confirm('Убрать «' + a.name + '» в корзину моста? Папку или пакет можно будет вернуть оттуда.')) return;
    closeApp(a.id);
    call('apps/remove', { id: a.id }, 'Убрано в корзину моста');
  }
  async function runAction(a: AppInfo, x: any) {
    if (x.confirm && !confirm(typeof x.confirm === 'string' ? x.confirm : 'Выполнить «' + x.title + '»?')) return;
    const k = a.id + '/' + x.id;
    actOut[k] = { busy: true, out: '', ok: true };
    try { const r = await api('POST', 'apps/action', { id: a.id, action: x.id, params: params[k] || {} }); actOut[k] = { busy: false, out: r.out || (r.ok ? 'Готово' : 'Код ' + r.code), ok: r.ok }; }
    catch (e: any) { actOut[k] = { busy: false, out: e.message, ok: false }; }
  }
  // рамки: сообщения от приложений
  function frame(node: HTMLIFrameElement, id: string) {
    const h = (e: MessageEvent) => onFrameMessage(id, node, e);
    window.addEventListener('message', h);
    return { destroy() { window.removeEventListener('message', h); closeNet(node); } };
  }
  // тема панели — во все открытые приложения
  let lastTheme = SET.theme;
  $effect(() => { const t = SET.theme; if (t !== lastTheme) { lastTheme = t; for (const id of AP.open) emitTo(id, 'theme', t); } });
  // перетащили файл: пакет .most-app — в папку приложений; другой файл на открытое приложение — ему (событие file)
  function onDrop(e: DragEvent) {
    dropHover = false;
    const files = [...(e.dataTransfer?.files || [])];
    if (!files.length) return;
    e.preventDefault();
    const pk = files.filter(f => /\.most-app$/i.test(f.name));
    if (pk.length) { importPackages(pk); return; }
    if (AP.view === 'app' && act && act.ui) for (const f of files) emitTo(act.id, 'file', f);
  }
  function tileDrop(e: DragEvent, a: AppInfo) {
    if (drag && drag !== a.id) { e.preventDefault(); e.stopPropagation(); reorder(drag, a.id); }
    drag = ''; over = '';
  }
</script>

<svelte:window onclick={() => (menu = null)} onkeydown={e => { if (e.key === 'Escape') { menu = null; logOf = null; } }} />

<div class="apps" role="region" aria-label="Приложения" ondragover={e => { if ([...(e.dataTransfer?.types || [])].includes('Files')) { e.preventDefault(); dropHover = true; } }} ondragleave={() => (dropHover = false)} ondrop={onDrop}>
  {#if AP.view === 'app' && act}
    <div class="apptabs">
      <button class="b sm" onclick={() => (AP.view = 'desk')}>‹ Все приложения</button>
      {#each AP.open as id (id)}
        {@const a = byId(id)}
        {#if a}
          <span class="atab" class:on={id === AP.active}>
            <button class="atn" onclick={() => (AP.active = id)}><i class="adot" style="background:hsl({hue(id)} 70% 60%)"></i>{AP.titles[id] || a.name}{#if AP.badges[id]}<b class="cnt red">{AP.badges[id]}</b>{/if}</button>
            <button class="atx" aria-label="Закрыть вкладку" title="Закрыть" onclick={() => closeApp(id)}>×</button>
          </span>
        {/if}
      {/each}
      <div class="grow"></div>
      {#if act.ui && act.state === 'ok'}
        <button class="b sm" title="Перезагрузить интерфейс" onclick={() => reloadApp(act.id)}>⟳ Перезапустить</button>
        <button class="b sm pri" title="В отдельное окно — его можно унести на другой монитор" onclick={() => popout(act.id)}><Gi n="pop" /> В окно</button>
      {/if}
      <button class="b sm" onclick={() => showLog(act.id)}>Журнал</button>
    </div>
    <div class="small muted astat">
      {act.version}{act.packaged ? ' · пакет' : ' · папка'}{act.runningService ? ' · служба работает, порт ' + act.runningService.port : act.service ? ' · служба запустится при обращении' : ''}{act.runningNative ? ' · своё окно запущено' : ''}{act.lastExit ? ' · ' + act.lastExit : ''}
      {#if AP.stale[act.id]}<button class="b sm" style="margin-left:8px" onclick={() => reloadApp(act.id)}>Есть новая версия — перезапустить</button>{/if}
    </div>
  {/if}

  <!-- рамки открытых веб-приложений живут, пока вкладка открыта (и при переходе на другие разделы моста) -->
  {#each uiOpen as a (a.id + ':' + (AP.gen[a.id] || 0))}
    <iframe class="appframe" class:hide={AP.view !== 'app' || AP.active !== a.id} data-app={a.id} title={a.name} src={frameSrc(a)} sandbox={sandboxOf(a)} allow={allowOf(a)} use:frame={a.id}></iframe>
  {/each}

  {#if AP.view === 'app' && act}
    {#if act.state !== 'ok'}
      <div class="empty">«{act.name}» не разрешено на этом ПК. <button class="b sm pri" onclick={() => openModal(AppConsent, { id: act.id, then: true })}>Разрешить…</button></div>
    {:else if !act.ui}
      <div class="card apanel">
        {#if act.native}
          <div class="row"><h3 style="margin:0">{act.name}</h3><span class="chip">{act.runningNative ? 'запущено в своём окне' : 'не запущено'}</span><div class="grow"></div>
            {#if act.runningNative}<button class="b sm danger" onclick={() => call('apps/stop', { id: act.id, kind: 'native' })}>Закрыть приложение</button>
            {:else}<button class="b sm pri" onclick={() => call('apps/start', { id: act.id, kind: 'native' })}>Запустить</button>{/if}
          </div>
          <div class="small muted" style="margin-top:6px">Приложение работает в своём окне. Здесь — его состояние; сеанс оно начинает само — с теми, кто в вашем голосовом канале.</div>
        {:else if act.net && !act.actions.length}
          <div class="row"><h3 style="margin:0">{act.name}</h3><span class="chip">сетевое</span></div>
          <div class="small muted" style="margin-top:6px">У приложения нет окна в мосте: к нему подключаются из редактора (Unity: «Мост → Подключение») или из своей программы. Сеансы — с теми, кто в вашем голосовом канале.</div>
        {/if}
        {#if act.actions.length}
          <div class="plbl" style="margin-top:10px">Кнопки</div>
          {#each act.actions as x (x.id)}
            {@const k = act.id + '/' + x.id}
            <div class="arow">
              {#each x.params || [] as p (p.id)}
                <label class="f small">{p.title || p.id}
                  {#if p.type === 'select'}<select value={params[k]?.[p.id] ?? p.default ?? ''} onchange={e => { (params[k] = params[k] || {})[p.id] = (e.target as HTMLSelectElement).value; }}>{#each p.options || [] as o}<option value={o}>{o}</option>{/each}</select>
                  {:else}<input type={p.type === 'number' ? 'number' : 'text'} value={params[k]?.[p.id] ?? p.default ?? ''} oninput={e => { (params[k] = params[k] || {})[p.id] = (e.target as HTMLInputElement).value; }} />{/if}
                </label>
              {/each}
              <button class="b pri" disabled={actOut[k]?.busy} onclick={() => runAction(act, x)}>{actOut[k]?.busy ? 'Выполняю…' : x.title}</button>
            </div>
            {#if actOut[k]?.out}<pre class="aout" class:bad={!actOut[k].ok}>{actOut[k].out}</pre>{/if}
          {/each}
        {/if}
      </div>
    {/if}
  {:else}
    <div class="row abar">
      <input class="aq" bind:value={q} placeholder="Найти приложение…" aria-label="Поиск приложения" />
      <button class="b" data-open="apps"><Gi n="folder" /> Папка приложений</button>
      {#if AP.open.length}<button class="b pri" onclick={() => (AP.view = 'app')}>Открытые · {AP.open.length}</button>{/if}
      <span class="small muted grow" style="text-align:right">Щелчок — открыть · правая кнопка — меню · перетащите, чтобы переставить</span>
    </div>
    {#if liveList.length}
      <div class="card alive">
        <div class="plbl" style="margin:0 0 6px">Идёт сейчас в канале «{live.channel.title}»</div>
        {#each liveList as s (s.sid)}
          {@const a = byId(s.app)}
          <div class="row small alrow">
            <span class="appic sm" style="--h:{hue(s.app)}">{#if a?.icon}<img src={'/apps/' + encodeURIComponent(s.app) + '/icon'} alt="" />{:else}{(a?.name || s.app).slice(0, 2)}{/if}</span>
            <b>{a?.name || s.app}</b><span class="muted">хост {titleOf(s.host)} · {s.count}/{s.max}{s.transport === 'external' ? ' · свой сервер' : ''}{s.members?.length ? ' · ' + s.members.join(', ') : ''}</span>
            <div class="grow"></div>
            {#if !a}<span class="muted">нет у вас</span>{:else if AP.open.includes(a.id)}<button class="b sm" onclick={() => { AP.active = a.id; AP.view = 'app'; }}>Открыто</button>{:else}<button class="b sm pri" onclick={() => joinLive(s)}>Войти</button>{/if}
          </div>
        {/each}
      </div>
    {/if}
    {#if AP.errors.length}
      <div class="card aerr">
        {#each AP.errors as e (e.source)}<div class="small"><b>{e.source}</b> — не подхватилось: {e.error}</div>{/each}
      </div>
    {/if}
    {#if !AP.loaded}<div class="empty">Загружаю…</div>
    {:else if !AP.list.length}
      <div class="card aempty">
        <h3 style="margin:0 0 6px">Приложений пока нет</h3>
        <div class="small">Перетащите пакет <b>.most-app</b> в это окно или положите папку приложения (с app.json) в <span class="mono">{AP.folder}</span> — мост подхватит её сам, без перезапуска.</div>
        <div class="row" style="margin-top:10px"><button class="b" data-open="apps"><Gi n="folder" /> Открыть папку приложений</button></div>
      </div>
    {:else}
      <div class="agrid">
        {#each list as a (a.id)}
          {@const [bt, bk] = badge(a)}
          <button class="atile" class:over={over === a.id} class:dim={a.state === 'unsupported'} draggable="true" title={a.description || a.name}
            ondragstart={e => { drag = a.id; e.dataTransfer?.setData('text/plain', a.id); }} ondragend={() => { drag = ''; over = ''; }}
            ondragover={e => { if (drag) { e.preventDefault(); over = a.id; } }} ondrop={e => tileDrop(e, a)}
            onclick={() => open(a)} oncontextmenu={e => ctx(e, a)}>
            <span class="appic" style="--h:{hue(a.id)}">{#if a.icon}<img src={'/apps/' + encodeURIComponent(a.id) + '/icon?v=' + a.uiStamp} alt="" />{:else}{a.name.slice(0, 2)}{/if}{#if AP.open.includes(a.id)}<i class="aopen"></i>{/if}</span>
            <span class="aname">{a.name}</span>
            <span class="abadge {bk}">{bt}</span>
          </button>
        {/each}
      </div>
      {#if !list.length}<div class="empty">Ничего не нашлось</div>{/if}
    {/if}
  {/if}
  {#if dropHover}<div class="adrop">Отпустите: пакет .most-app добавится в приложения, другой файл — откроется в приложении</div>{/if}
</div>

{#if menu}
  {@const a = byId(menu.id)}
  {#if a}
    <div class="amenu" role="menu" tabindex="-1" style="left:{Math.min(menu.x, innerWidth - 250)}px;top:{Math.min(menu.y, innerHeight - 340)}px" onclick={e => e.stopPropagation()} onkeydown={() => {}}>
      <button role="menuitem" onclick={() => open(a)}>Открыть</button>
      {#if a.ui && a.state === 'ok'}<button role="menuitem" onclick={() => { menu = null; openApp(a.id); popout(a.id); }}>Открыть в отдельном окне</button>{/if}
      {#if a.native && a.state === 'ok'}{#if a.runningNative}<button role="menuitem" onclick={() => call('apps/stop', { id: a.id, kind: 'native' })}>Закрыть своё окно</button>{:else}<button role="menuitem" onclick={() => call('apps/start', { id: a.id, kind: 'native' })}>Запустить своё окно</button>{/if}{/if}
      {#if a.service && a.state === 'ok'}{#if a.runningService}<button role="menuitem" onclick={() => call('apps/stop', { id: a.id, kind: 'service' }, 'Служба остановлена')}>Остановить службу</button>{:else}<button role="menuitem" onclick={() => call('apps/start', { id: a.id, kind: 'service' }, 'Служба запущена')}>Запустить службу</button>{/if}{/if}
      <button role="menuitem" onclick={() => showLog(a.id)}>Журнал</button>
      <button role="menuitem" onclick={() => { menu = null; openModal(AppConsent, { id: a.id }); }}>Права и о приложении…</button>
      <button role="menuitem" data-open="apps" onclick={() => (menu = null)}>Показать папку приложений</button>
      {#if a.state === 'ok'}<button role="menuitem" onclick={() => { closeApp(a.id); call('apps/revoke', { id: a.id }, 'Разрешение отозвано'); }}>Отозвать разрешение</button>{/if}
      <i class="msep"></i>
      <button role="menuitem" class="danger" onclick={() => remove(a)}>Убрать (в корзину моста)</button>
    </div>
  {/if}
{/if}

{#if logOf}
  <div class="alog card">
    <div class="row"><b>Журнал · {byId(logOf.id)?.name || logOf.id}</b><div class="grow"></div><button class="b sm" onclick={() => showLog(logOf!.id)}>Обновить</button><button class="b sm" onclick={() => (logOf = null)}>✕</button></div>
    <pre>{logOf.lines.length ? logOf.lines.join('\n') : 'Пусто: служба и своё окно ещё не запускались.'}</pre>
  </div>
{/if}

<style>
  .apps { position: relative; min-height: 60vh; display: flex; flex-direction: column; }
  .abar { gap: 10px; flex-wrap: wrap; margin-bottom: 16px; }
  .aq { flex: 1 1 260px; max-width: 420px; }
  .agrid { display: grid; grid-template-columns: repeat(auto-fill, minmax(124px, 1fr)); gap: 16px 10px; align-content: start; }
  .atile { display: flex; flex-direction: column; align-items: center; gap: 7px; padding: 12px 6px 10px; border-radius: 14px; border: 1px solid transparent; background: transparent; color: inherit; font: inherit; cursor: pointer; }
  .atile:hover, .atile:focus-visible { background: var(--panel2); border-color: var(--line); }
  .atile.over { border-color: var(--acc, #38E1F0); }
  .atile.dim { opacity: .55; }
  .aopen { position: absolute; bottom: -7px; left: 50%; width: 6px; height: 6px; margin-left: -3px; border-radius: 3px; background: var(--acc, #38E1F0); }
  .aname { font-size: 13px; font-weight: 600; text-align: center; line-height: 1.25; }
  .abadge { font-size: 11px; line-height: 1; padding: 3px 7px; white-space: nowrap; max-width: 100%; overflow: hidden; text-overflow: ellipsis; border-radius: 8px; background: var(--panel2); color: var(--muted); }
  .abadge.good { background: rgba(93,224,142,.14); color: #8FEBB0; }
  .abadge.warn { background: rgba(255,181,71,.18); color: #FFD08A; }
  .apptabs { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; border-bottom: 1px solid var(--line); padding-bottom: 8px; }
  .atab { display: inline-flex; align-items: center; border-radius: 10px; border: 1px solid var(--line); }
  .atab.on { border-color: var(--acc, #38E1F0); background: var(--panel2); }
  .atn { height: 32px; padding: 0 6px 0 12px; border: 0; background: transparent; color: inherit; font: inherit; font-size: 13px; font-weight: 600; cursor: pointer; display: inline-flex; align-items: center; gap: 8px; }
  .atx { width: 26px; height: 32px; border: 0; background: transparent; color: var(--muted); font-size: 15px; cursor: pointer; }
  .adot { width: 8px; height: 8px; border-radius: 4px; display: inline-block; }
  .astat { padding: 6px 2px 10px; min-height: 18px; }
  .appframe { width: 100%; flex: 1; min-height: 70vh; border: 1px solid var(--line); border-radius: 14px; background: #fff0; }
  .apanel { max-width: 760px; }
  .arow { display: flex; flex-wrap: wrap; gap: 10px; align-items: flex-end; margin-top: 8px; }
  .aout { margin: 6px 0 0; padding: 8px 10px; background: var(--panel2); border-radius: 8px; max-height: 240px; overflow: auto; white-space: pre-wrap; font-size: 12px; }
  .aout.bad { color: var(--warn); }
  .aerr { margin-bottom: 12px; border-color: rgba(255,181,71,.4); }
  .alive { margin-bottom: 14px; border-color: rgba(56,225,240,.3); }
  .alrow { gap: 10px; align-items: center; padding: 4px 0; }
  .aempty { max-width: 640px; }
  .adrop { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; text-align: center; padding: 20px; border: 2px dashed var(--acc, #38E1F0); border-radius: 16px; background: rgba(11,12,26,.7); pointer-events: none; font-weight: 600; }
  .amenu { position: fixed; z-index: 30; width: 240px; background: var(--panel, #171935); border: 1px solid var(--line); border-radius: 12px; padding: 6px; box-shadow: 0 12px 32px rgba(0,0,0,.45); display: flex; flex-direction: column; }
  .amenu button { text-align: left; padding: 8px 12px; border: 0; border-radius: 8px; background: transparent; color: inherit; font: inherit; font-size: 13px; cursor: pointer; }
  .amenu button:hover { background: var(--panel2); }
  .amenu .danger { color: var(--bad, #FF8C9A); }
  .msep { height: 1px; background: var(--line); margin: 4px 6px; }
  .alog { position: fixed; right: 20px; bottom: 20px; width: min(640px, calc(100vw - 40px)); z-index: 25; }
  .alog pre { margin: 8px 0 0; max-height: 320px; overflow: auto; font-size: 12px; white-space: pre-wrap; }
</style>
