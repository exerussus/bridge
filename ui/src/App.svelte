<script lang="ts">
  import { ui, TABS, TONE, HEAD, setTab, load, toast, closeModal, SET, setSet, setTitle, openModal, titleOf } from './lib/state.svelte';
  import { api, IS_APP } from './lib/api';
  import { ago, size, dur } from './lib/util';
  import { TAB_ICONS } from './lib/icons';
  import { chStart, CHT } from './lib/chat.svelte';
  import { asStart } from './lib/assistant.svelte';
  import { voiceStart, R } from './voice/engine.svelte';
  import './voice/screen.svelte';
  import Sidebar from './components/Sidebar.svelte';
  import TopBar from './components/TopBar.svelte';
  import Xfers from './components/Xfers.svelte';
  import Palette from './modals/Palette.svelte';
  import { netStart } from './lib/netstat.svelte';
  import { toggleMic, toggleDeaf } from './voice/engine.svelte';
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
  import Apps from './tabs/Apps.svelte';
  import BuildsTab from './tabs/BuildsTab.svelte';

  const WIRE: Record<string, number> = { cyan: 0, red: 1, amber: 2, green: 3, dim: -1 };

  const ov = $derived(ui.ov);
  const live = $derived(ov ? ov.peers.filter((p: any) => p.enabled && !p.invite) : []);
  const on = $derived(live.filter((p: any) => p.online).length);
  const inbox = $derived(ov ? ov.shares.reduce((a: number, s: any) => a + s.inbox, 0) + ov.waiting.length : 0);
  const pend = $derived(ov ? (Object.values(ov.mailbox.pending || {}).reduce((a: number, p: any) => a + p.files, 0) as number) : 0);
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

  // ── запуск ──
  let syncing = $state(false);
  async function sync() { syncing = true; try { await api('POST', 'sync'); toast('Обмен выполнен', 'ok'); await load(); } catch (e: any) { toast('Обмен: ' + e.message, 'bad'); } syncing = false; }
  $effect(() => {
    const h = location.hash.slice(1);
    setTab(TABS.includes(h as any) ? h : 'home');
    load(); voiceStart(); chStart(); asStart(); netStart();
    const t = setInterval(() => { if (!document.hidden) load(); }, 3000);
    return () => clearInterval(t);
  });
  // ── клавиши ──
  function keys(e: KeyboardEvent) {
    if (e.key === 'Escape' && ui.modal) { closeModal(); return; }
    // (3.21) Ctrl K — поиск-переход, Ctrl M — микрофон, Ctrl D — звук (и в полях ввода)
    if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey) {
      const k = e.code;
      if (k === 'KeyK') { e.preventDefault(); if (!ui.modal) openModal(Palette); return; }
      if (k === 'KeyM') { e.preventDefault(); toggleMic(); return; }
      if (k === 'KeyD') { e.preventDefault(); toggleDeaf(); return; }
    }
    const t = e.target as HTMLElement;
    if (t.matches('input,textarea,select')) { if (e.key === 'Escape') t.blur(); return; }
    if (ui.modal || (ui.tab === 'voice' && document.body.classList.contains('bdfull'))) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === 's' || e.key === 'ы') sync();
    const n = '123456789'.indexOf(e.key);
    if (n >= 0 && !(ui.tab === 'voice' && R.key)) setTab(['home', 'peers', 'voice', 'chat', 'send', 'builds', 'folders', 'shares', 'records'][n]);
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
  <Sidebar />
  <div class="content">
    <header><span class="wire" style="opacity:{WIRE[tone] < 0 ? 0 : 1};left:{WIRE[tone] >= 0 ? WIRE[tone] * 25 : 0}%"></span>
      <div class="top"><TopBar /></div>
    </header>
    <main>
      <div class="pagehead"><h1>{HEAD[ui.tab]?.[0] || ''}</h1><hr /><p>{HEAD[ui.tab]?.[1] || ''}</p></div>
      {#if ['settings', 'access', 'log', 'help'].includes(ui.tab)}
        <!-- (3.21) права, журнал и справка — внутри «Настроек», а не отдельными пунктами меню -->
        <div class="vtabs" role="tablist" aria-label="Настройки" style="margin:-8px 0 18px">
          {#each [['settings', 'Этот ПК'], ['access', 'Права и группы'], ['log', 'Журнал'], ['help', 'Справка']] as [k, l]}<button role="tab" class:on={ui.tab === k} onclick={() => setTab(k)}>{l}</button>{/each}
        </div>
      {/if}
      {#if ui.ready}
        <section id="t-{ui.tab}" class:hide={ui.tab === 'voice' || ui.tab === 'apps'}>
          {#if ui.tab === 'home'}<Home />
          {:else if ui.tab === 'peers'}<Peers />
          {:else if ui.tab === 'send'}<Send />
          {:else if ui.tab === 'builds'}<BuildsTab />
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
        <!-- приложения тоже всегда в панели: открытые не перезагружаются при переходе на другие вкладки -->
        <section id="t-apps" class:hide={ui.tab !== 'apps'}><Apps /></section>
      {:else}
        <div class="empty">{ui.conn ? 'Мост не отвечает — запущен ли он?' : 'Загружаю…'}</div>
      {/if}
    </main>
  </div>
</div>

<Xfers />
<div class="modal" class:on={!!ui.modal} role="presentation"
  onpointerdown={e => (downOnBack = e.target === e.currentTarget)}
  onclick={e => { if (e.target === e.currentTarget && downOnBack) closeModal(); downOnBack = false; }}>
  <div class="card">
    {#if ui.modal}{@const M = ui.modal.c}<M {...(ui.modal?.p || {})} />{/if}
  </div>
</div>
<div class="toast" use:followFs>{#each ui.toasts as t (t.id)}<div class={t.kind}>{t.msg}</div>{/each}</div>
<UpdCard />
<ControlUI />
<Lightbox />
