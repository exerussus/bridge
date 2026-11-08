<script lang="ts">
  // (3.21) Боковая панель: три группы, счётчики только полезные. Зелёным — сколько соседей НА СВЯЗИ,
  // красным — только эфир в каналах; янтарным — только то, что ждёт решения. Остальное — серым.
  import { ui, setTab, openModal, SET } from '../lib/state.svelte';
  import { TAB_ICONS } from '../lib/icons';
  import { NS, relayLine, netLine } from '../lib/netstat.svelte';
  import Changelog from '../modals/Changelog.svelte';
  import Palette from '../modals/Palette.svelte';

  const ov = $derived(ui.ov);
  const live = $derived(ov ? ov.peers.filter((p: any) => p.enabled && !p.invite) : []);
  const on = $derived(live.filter((p: any) => p.online).length);
  const pend = $derived(ov ? (Object.values(ov.mailbox?.pending || {}).reduce((a: number, p: any) => a + (p.files || 0), 0) as number) : 0);
  const waiting = $derived(ov ? ov.waiting.length : 0);

  type B = { t: string; c: string } | null;
  const badge = $derived.by(() => {
    const c = ui.counts;
    const m: Record<string, B> = {
      home: waiting ? { t: waiting + ' ждёт', c: 'amb' } : null,
      peers: live.length ? { t: String(on), c: on ? 'ok dotb' : '' } : null,
      voice: c.voice ? { t: String(c.voice), c: 'live dotb' } : null,
      chat: c.chat ? { t: String(c.chat), c: '' } : null,
      send: pend ? { t: '↑ ' + pend, c: '' } : null,
      folders: c.folders ? { t: c.folders + ' ждёт', c: 'amb' } : null,
      agent: c.agent ? { t: String(c.agent), c: '' } : null,
      assistant: c.assistant ? { t: String(c.assistant), c: '' } : null,
      apps: c.apps ? { t: String(c.apps), c: '' } : null,
      phone: c.phone ? { t: String(c.phone), c: '' } : null,
    };
    return m;
  });
  const GROUPS: [string, [string, string][]][] = [
    ['', [['home', 'Обзор']]],
    ['Связь', [['peers', 'Соседи'], ['voice', 'Каналы'], ['chat', 'Чаты']]],
    ['Файлы', [['send', 'Отправка'], ['builds', 'Сборки'], ['folders', 'Общие папки'], ['shares', 'Пространства']]],
    ['Работа', [['records', 'Записи'], ['agent', 'Агенты'], ['assistant', 'Ассистент'], ['apps', 'Приложения']]],
  ];
  const SETTINGS = ['settings', 'access', 'log', 'help'];
  const isOn = (k: string) => (k === 'settings' ? SETTINGS.includes(ui.tab) : ui.tab === k);
  const rl = $derived((void NS.at, relayLine()));
  const nl = $derived((void NS.at, netLine()));
</script>

<aside class="sb">
  <div class="brand"><svg class="logo" viewBox="0 0 40 40" aria-hidden="true">
    <path class="arc" d="M5 29c6-13 24-13 30 0" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" />
    <path class="end" d="M12 29v-6M20 29v-9M28 29v-6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" opacity=".55" />
    <path class="end" d="M3 29h34" stroke="currentColor" stroke-width="1" opacity=".3" />
    <circle class="end" cx="5" cy="29" r="2.2" fill="#38E1F0" style="filter:drop-shadow(0 0 4px #38E1F0)" /><circle class="end" cx="35" cy="29" r="2.2" fill="#FF4F64" style="filter:drop-shadow(0 0 4px #FF4F64)" /></svg>
    <div class="wordmark"><b>Мост</b><span class="sub">{ov?.node.node || ''}</span></div></div>

  <button class="sb-cmd" onclick={() => openModal(Palette)} title="Перейти к разделу, соседу или действию">
    <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.5" /><path d="M16 16l4.5 4.5" /></svg>
    <span class="ell">Найти, перейти…</span><kbd>Ctrl K</kbd>
  </button>

  <nav class="sb-nav" id="tabs">
    {#each GROUPS as [g, items]}
      {#if g}<div class="sb-grp">{g}</div>{/if}
      {#each items as [k, t]}
        {@const b = badge[k]}
        <button class="sb-it" class:on={isOn(k)} onclick={() => setTab(k)}>
          <svg viewBox="0 0 24 24" aria-hidden="true">{@html TAB_ICONS[k]}</svg><span class="sb-t">{t}</span>
          {#if b}<span class="sb-b {b.c}">{#if b.c.includes('dotb')}<i></i>{/if}{b.t}</span>{/if}
        </button>
      {/each}
    {/each}
  </nav>

  <div class="sb-foot">
    <button class="sb-it" class:on={isOn('phone')} onclick={() => setTab('phone')}><svg viewBox="0 0 24 24" aria-hidden="true">{@html TAB_ICONS.phone}</svg><span class="sb-t">Телефон</span>{#if badge.phone}<span class="sb-b">{badge.phone.t}</span>{/if}</button>
    <button class="sb-it" class:on={isOn('settings')} onclick={() => setTab('settings')}><svg viewBox="0 0 24 24" aria-hidden="true">{@html TAB_ICONS.settings}</svg><span class="sb-t">Настройки</span></button>
    <div class="sb-stat">
      <button class="sb-row" onclick={() => { ui.peersSub = 'net'; setTab('peers'); }} title="Свой сервер — «Соседи» → «Сеть и сервер»"><span class="dot {rl.dot}"></span><span class="grow ell">{rl.text}</span><span class="sb-dim">{rl.sub}</span></button>
      <button class="sb-row" onclick={() => { ui.peersSub = 'net'; setTab('peers'); }} title="Своя сеть MostNet"><span class="dot {nl.dot}"></span><span class="grow ell">{nl.text}</span><span class="sb-dim ell">{nl.sub}</span></button>
      {#if ov}<button class="sb-row sb-ver" onclick={() => openModal(Changelog)} title="Что нового"><span class="grow">v{ov.node.version}{#if SET.seenVersion !== ov.node.version}<i class="vdot"></i>{/if}</span><span class="sb-new">Что нового ›</span></button>{/if}
    </div>
  </div>
</aside>
