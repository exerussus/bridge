<script lang="ts">
  // Раздел «Сборки» (3.8): присланные мне (последняя версия, история, запуск) и мои заготовки (отправка новых версий).
  import { SET, setSet } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import Builds from '../components/Builds.svelte';
  import ReceivedBuilds from '../components/ReceivedBuilds.svelte';
  import Gi from '../components/Gi.svelte';
  let B = $state<any>(null);
  let tab = $state<string>(SET.buildsTab || '');
  async function load() { try { B = await api('GET', 'builds'); if (!tab) tab = B.received.length ? 'in' : 'mine'; } catch {} }
  $effect(() => {
    let t: any = 0, live = true;
    const tick = async () => { await load(); if (live) t = setTimeout(tick, B && (B.jobs.length || B.received.some((r: any) => r.running)) ? 2000 : 5000); };
    tick();
    return () => { live = false; clearTimeout(t); };
  });
  function setTab(t: string) { tab = t; setSet('buildsTab', t); }
</script>

<div class="vtabs sttabs" role="tablist" aria-label="Сборки" style="margin-bottom:14px">
  <button role="tab" class:on={tab === 'in'} onclick={() => setTab('in')}><Gi n="down" /> Присланные мне{#if B?.received.length}&nbsp;<span class="muted">· {B.received.length}</span>{/if}</button>
  <button role="tab" class:on={tab === 'mine'} onclick={() => setTab('mine')}><Gi n="up" /> Мои сборки{#if B?.presets.length}&nbsp;<span class="muted">· {B.presets.length}</span>{/if}</button>
</div>
{#if tab === 'mine'}
  <Builds />
{:else if B}
  <ReceivedBuilds {B} reload={load} />
{:else}
  <div class="empty">Загружаю…</div>
{/if}
