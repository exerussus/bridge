<script lang="ts">
  import { ui, TABS, TONE, HEAD, setTab, load, toast, closeModal, SET, setSet, setTitle, openModal, titleOf } from './lib/state.svelte';
  import { api, IS_APP } from './lib/api';
  import { ago, size, dur } from './lib/util';
  import { TAB_ICONS } from './lib/icons';
  import { chStart, CHT } from './lib/chat.svelte';
  import { asStart } from './lib/assistant.svelte';
  import { voiceStart, R } from './voice/engine.svelte';
  import './voice/screen.svelte';
  import Gi from './components/Gi.svelte';
  import UpdCard from './components/UpdCard.svelte';
  import ControlUI from './components/ControlUI.svelte';
  import Lightbox from './components/Lightbox.svelte';
  import Changelog from './modals/Changelog.svelte';
  import Home from './tabs/Home.svelte';
  import Peers from './tabs/Peers.svelte';
  import Send from './tabs/Send.svelte';
  import Folders from './tabs/Folders.svelte';
  import Shares from './tabs/Shares.svelte';
  import Records from './tabs/Records.svelte';
  import Chats from './tabs/Chats.svelte';
  import Agent from './tabs/Agent.svelte';
  import Assistant from './tabs/Assistant.svelte';
  import Phone from './tabs/Phone.svelte';
  import Settings from './tabs/Settings.svelte';
  import Access from './tabs/Access.svelte';
  import Log from './tabs/Log.svelte';
  import Help from './tabs/Help.svelte';
  import Voice from './voice/Voice.svelte';

  const NAV: (string | [string, string])[] = [
    ['home', 'Обзор'], 'Мост', ['peers', 'Соседи'], ['send', 'Отправка'], ['folders', 'Общие папки'], ['shares', 'Пространства'], ['records', 'Записи'],
    'Связь', ['voice', 'Каналы'], ['agent', 'Агенты'], ['chat', 'Чаты'], ['assistant', 'Ассистент'], ['phone', 'Телефон'],
    'Настройка', ['settings', 'Настройки'], ['access', 'Права и группы'], ['log', 'Журнал'], ['help', 'Справка'],
  ];
  const WIRE: Record<string, number> = { cyan: 0, red: 1, amber: 2, green: 3, dim: -1 };

  const ov = $derived(ui.ov);
  const live = $derived(ov ? ov.peers.filter((p: any) => p.enabled && !p.invite) : []);
  const on = $derived(live.filter((p: any) => p.online).length);
  const inbox = $derived(ov ? ov.shares.reduce((a: number, s: any) => a + s.inbox, 0) + ov.waiting.length : 0);
  const pend = $derived(ov ? (Object.values(ov.mailbox.pending || {}).reduce((a: number, p: any) => a + p.files, 0) as number) : 0);
  const counts = $derived({ ...ui.counts, records: inbox, send: pend, peers: live.length - on } as Record<string, number>);
  const tone = $derived(TONE[ui.tab] || 'cyan');
  $effect(() => { document.body.dataset.tone = tone; document.body.dataset.tab = ui.tab; });
  $effect(() => {
    if (!ov) return;
    const fm = ui.forcedBy ? '🔇 ВАС ЗАГЛУШИЛИ · ' : '';
    setTitle(fm + (inbox > 0 ? '(' + inbox + ') ' : '') + 'Мост · ' + ov.node.node);
  });
  // уведомление о новом в записях, когда окно свёрнуто
  let prevAtt: number | null = null;
  $effect(() => {
    const n = inbox;
    if (prevAtt !== null && n > prevAtt && document.hidden && ov) import('./lib/notify').then(m => m.notifyTo('Мост · ' + ov.node.node, ov.waiting.length ? 'Команда ждёт вашего подтверждения' : 'Новое в записях: ' + n, 'records'));
    prevAtt = n;
  });

  // ── передачи: плашка в углу на любой вкладке ──
  let xfers = $state<any[]>([]);
  $effect(() => { xfers = ov?.transfers || []; });
  $effect(() => {
    if (!xfers.length) return;
    const t = setTimeout(async () => { if (document.hidden) return; try { xfers = await api('GET', 'transfers'); } catch {} }, 1000);
    return () => clearTimeout(t);
  });
  const XK: Record<string, string> = { up: '⬆', down: '⬇', serve: '⬆', recv: '⬇' };
  const who = (x: any) => (x.kind === 'up' ? 'отправляю ' + titleOf(x.peer) : x.kind === 'down' ? 'скачиваю у ' + titleOf(x.peer) : x.kind === 'serve' ? titleOf(x.peer) + ' забирает' : titleOf(x.peer) + ' присылает');

  // ── запуск ──
  let syncing = $state(false);
  async function sync() { syncing = true; try { await api('POST', 'sync'); toast('Обмен выполнен', 'ok'); await load(); } catch (e: any) { toast('Обмен: ' + e.message, 'bad'); } syncing = false; }
  async function toggleNotify() {
    if (IS_APP) { setSet('notify', !SET.notify); toast(SET.notify ? 'Уведомления включены' : 'Уведомления выключены'); return; }
    if (!('Notification' in window)) { toast('Браузер не поддерживает уведомления', 'bad'); return; }
    if (!SET.notify && Notification.permission !== 'granted') { const p = await Notification.requestPermission(); if (p !== 'granted') { toast('Уведомления запрещены в браузере', 'bad'); return; } }
    setSet('notify', !SET.notify);
    toast(SET.notify ? 'Уведомления включены' : 'Уведомления выключены');
  }
  function theme() { const next = (document.documentElement.dataset.theme || 'dark') === 'dark' ? 'light' : 'dark'; document.documentElement.dataset.theme = next; setSet('theme', next); }

  $effect(() => {
    const h = location.hash.slice(1);
    setTab(TABS.includes(h as any) ? h : 'home');
    load(); voiceStart(); chStart(); asStart();
    const t = setInterval(() => { if (!document.hidden) load(); }, 3000);
    return () => clearInterval(t);
  });
  // ── клавиши ──
  function keys(e: KeyboardEvent) {
    if (e.key === 'Escape' && ui.modal) { closeModal(); return; }
    const t = e.target as HTMLElement;
    if (t.matches('input,textarea,select')) { if (e.key === 'Escape') t.blur(); return; }
    if (ui.modal || (ui.tab === 'voice' && document.body.classList.contains('bdfull'))) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === 's' || e.key === 'ы') sync();
    const n = '123456789'.indexOf(e.key);
    if (n >= 0 && !(ui.tab === 'voice' && R.key)) setTab(TABS[n]);
  }
  // открыть папку в Проводнике: [data-open] на любой кнопке
  async function opener(e: MouseEvent) {
    const o = (e.target as HTMLElement).closest('[data-open]') as HTMLElement | null;
    if (!o) return;
    e.preventDefault();
    const q = new URLSearchParams({ target: o.dataset.open! });
    for (const k of ['peer', 'share', 'id']) if (o.dataset[k]) q.set(k, o.dataset[k]!);
    try { await api('POST', 'open?' + q); } catch (err: any) { toast(err.message, 'bad'); }
  }
  // мягкое свечение вокруг мыши на мелких плитках
  function glow(e: PointerEvent) {
    const t = (e.target as HTMLElement).closest?.('.tile,.chan,.chanmini,.ci,.share-row,.ctl') as HTMLElement | null;
    if (!t) return;
    const r = t.getBoundingClientRect();
    t.style.setProperty('--mx', e.clientX - r.left + 'px');
    t.style.setProperty('--my', e.clientY - r.top + 'px');
  }
  // окно: закрывается только щелчком по фону — не тогда, когда кнопку нажали внутри, а отпустили снаружи
  let downOnBack = false;
  // всплывашки видны и во весь экран (показ развёрнут): контейнер переезжает внутрь развёрнутого элемента
  function followFs(node: HTMLElement) {
    const home = node.parentElement!, f = () => (document.fullscreenElement || home).appendChild(node);
    document.addEventListener('fullscreenchange', f);
    return { destroy: () => document.removeEventListener('fullscreenchange', f) };
  }
</script>

<svelte:window onkeydown={keys} />
<svelte:document onclick={opener} onpointermove={glow} />

<div class="app">
  <aside>
    <div class="brand"><svg class="logo" viewBox="0 0 40 40" aria-hidden="true">
      <path class="arc" d="M5 29c6-13 24-13 30 0" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" />
      <path class="end" d="M12 29v-6M20 29v-9M28 29v-6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" opacity=".55" />
      <path class="end" d="M3 29h34" stroke="currentColor" stroke-width="1" opacity=".3" />
      <circle class="end" cx="5" cy="29" r="2.2" fill="#38E1F0" style="filter:drop-shadow(0 0 4px #38E1F0)" /><circle class="end" cx="35" cy="29" r="2.2" fill="#FF4F64" style="filter:drop-shadow(0 0 4px #FF4F64)" /></svg>
      <div class="wordmark"><b>Мост</b><span class="sub">{ov?.node.node || ''}</span></div></div>
    <nav id="tabs">
      {#each NAV as it}
        {#if typeof it === 'string'}<div class="grp">{it}</div>
        {:else}
          <button class:on={ui.tab === it[0]} onclick={() => setTab(it[0])}><span class="ic"><svg viewBox="0 0 24 24">{@html TAB_ICONS[it[0]]}</svg></span>{it[1]}
            {#if it[0] !== 'home' && it[0] !== 'settings' && it[0] !== 'access' && it[0] !== 'log' && it[0] !== 'help' && it[0] !== 'shares'} <span class="cnt" class:hide={!counts[it[0]]}>{counts[it[0]] || ''}</span>{/if}</button>
        {/if}
      {/each}
    </nav>
    <div class="foot">
      {#if ov}<b>{ov.node.node}</b> <button class="verbtn" title="Что нового" onclick={() => openModal(Changelog)}>v{ov.node.version}{#if SET.seenVersion !== ov.node.version}<i class="vdot"></i>{/if}</button><br />
        {#each ov.announce || [] as u}{u.replace('http://', '')}<br />{/each}{/if}
    </div>
  </aside>
  <div class="content">
    <header><span class="wire" style="opacity:{WIRE[tone] < 0 ? 0 : 1};left:{WIRE[tone] >= 0 ? WIRE[tone] * 25 : 0}%"></span>
      <div class="top">
        {#if ov}
          <span class="pill {on === live.length ? 'ok' : on === 0 && live.length > 0 ? 'bad' : 'warn'}" title={ov.node.message || ''}>{live.length === 0 ? 'нет соседей' : on === live.length ? 'все на связи' : on === 0 ? 'нет связи с соседями' : 'на связи ' + on + ' из ' + live.length}</span>
          <span class="sub">запущен {ago(ov.node.started)} · соседи {on}/{live.length}</span>
        {:else}<span class="pill">…</span>{/if}
        <div class="grow"></div>
        <span class="sub">{#if ui.conn}<span class="pill bad">{ui.conn}</span>{/if}</span>
        <button class="b" title="Обменяться со всеми соседями сейчас (S)" disabled={syncing} onclick={sync}><Gi n="sync" /> Обменяться</button>
        <button class="b" style="opacity:{SET.notify ? 1 : 0.45}" title={SET.notify ? 'Уведомления включены (нажмите, чтобы выключить)' : 'Включить уведомления о звонках, сообщениях и задачах'} onclick={toggleNotify}><Gi n="bell" /></button>
        <button class="b" title="Светлая / тёмная тема" onclick={theme}><Gi n="theme" /></button>
      </div>
    </header>
    <main>
      <div class="pagehead"><h1>{HEAD[ui.tab]?.[0] || ''}</h1><hr /><p>{HEAD[ui.tab]?.[1] || ''}</p></div>
      {#if ui.ready}
        <section id="t-{ui.tab}" class:hide={ui.tab === 'voice'}>
          {#if ui.tab === 'home'}<Home />
          {:else if ui.tab === 'peers'}<Peers />
          {:else if ui.tab === 'send'}<Send />
          {:else if ui.tab === 'folders'}<Folders />
          {:else if ui.tab === 'shares'}<Shares />
          {:else if ui.tab === 'records'}<Records />
          {:else if ui.tab === 'chat'}<Chats />
          {:else if ui.tab === 'agent'}<Agent />
          {:else if ui.tab === 'assistant'}<Assistant />
          {:else if ui.tab === 'phone'}<Phone />
          {:else if ui.tab === 'settings'}<Settings />
          {:else if ui.tab === 'access'}<Access />
          {:else if ui.tab === 'log'}<Log />
          {:else if ui.tab === 'help'}<Help />{/if}
        </section>
        <!-- каналы всегда в панели: разговор, показы и доска живут, пока открыты другие вкладки -->
        <section id="t-voice" class:hide={ui.tab !== 'voice'}><Voice /></section>
      {:else}
        <div class="empty">{ui.conn ? 'Мост не отвечает — запущен ли он?' : 'Загружаю…'}</div>
      {/if}
    </main>
  </div>
</div>

<div id="xfers" class:hide={!xfers.length}>
  {#if xfers.length}
    <div class="row"><b class="small">Передачи: {xfers.length}</b></div>
    {#each xfers as x}
      {@const pct = x.total > 0 ? Math.min(100, Math.round((x.done * 100) / x.total)) : null}
      <div class="x">
        <div class="row small"><span>{XK[x.kind] || '⇅'}</span><b class="grow" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title={x.path}>{x.path.split('/').pop()}</b><span class="muted">{pct == null ? '' : pct + '%'}</span></div>
        <div class="bar"><i style="width:{pct == null ? 100 : pct}%{pct == null ? ';opacity:.4' : ''}"></i></div>
        <div class="small muted">{who(x)} · {x.what} · {size(x.done)}{x.total ? ' из ' + size(x.total) : ''} · {size(x.speed)}/с{x.eta >= 0 ? ' · осталось ' + dur(x.eta) : ''}</div>
      </div>
    {/each}
  {/if}
</div>
<div class="modal" class:on={!!ui.modal} role="presentation"
  onpointerdown={e => (downOnBack = e.target === e.currentTarget)}
  onclick={e => { if (e.target === e.currentTarget && downOnBack) closeModal(); downOnBack = false; }}>
  <div class="card">
    {#if ui.modal}{@const M = ui.modal.c}<M {...ui.modal.p} />{/if}
  </div>
</div>
<div class="toast" use:followFs>{#each ui.toasts as t (t.id)}<div class={t.kind}>{t.msg}</div>{/each}</div>
<UpdCard />
<ControlUI />
<Lightbox />
