<script lang="ts">
  // «Настройки» → «Приложения» (3.9): все приложения этого ПК, как в Android — разрешение, автозапуск, что работает.
  import { openModal } from '../lib/state.svelte';
  import { AP, ordered, loadApps, startApps } from '../lib/apps.svelte';
  import { size } from '../lib/util';
  import AppSettings from '../modals/AppSettings.svelte';
  import AppConsent from '../modals/AppConsent.svelte';
  $effect(() => { startApps(); loadApps(); });
  const hue = (s: string) => { let h = 0; for (const ch of s) h = (h * 31 + ch.charCodeAt(0)) >>> 0; return h % 360; };
  function chips(a: any): string[] {
    const r: string[] = [];
    if (a.state === 'new') r.push('не разрешено');
    else if (a.state === 'changed') r.push('изменилось — разрешить');
    else if (a.state === 'unsupported') r.push('не для этой системы');
    else {
      r.push(a.consent?.trust ? 'постоянно' : 'эта версия');
      if (a.serviceAlways || a.consent?.autostart?.service) r.push('служба с мостом');
      if (a.consent?.autostart?.native) r.push('окно с мостом');
    }
    if (a.runningService) r.push('служба работает');
    if (a.runningNative) r.push('окно открыто');
    return r;
  }
</script>

<div class="card"><h3>Приложения</h3>
  <div class="small muted" style="margin-bottom:8px">Как в телефоне: у каждого приложения — разрешение (на одну версию или постоянно), права, автозапуск вместе с мостом и данные. Приложения сами эти настройки менять не могут.</div>
  {#each ordered() as a (a.id)}
    <button class="asrow" onclick={() => openModal(a.state === 'ok' || a.state === 'unsupported' ? AppSettings : AppConsent, { id: a.id })}>
      <span class="appic sm" style="--h:{hue(a.id)}">{#if a.icon}<img src={'/apps/' + encodeURIComponent(a.id) + '/icon'} alt="" />{:else}{a.name.slice(0, 2)}{/if}</span>
      <span class="grow asn"><b>{a.name}</b> <span class="muted small">{a.version}{a.dataSize ? ' · ' + size(a.dataSize) : ''}</span></span>
      <span class="aschips">{#each chips(a) as c (c)}<span class="chip">{c}</span>{/each}</span>
      <span class="muted">›</span>
    </button>
  {:else}
    <div class="empty">Приложений пока нет — раздел «Приложения»</div>
  {/each}
</div>

<style>
  .asrow { display: flex; align-items: center; gap: 12px; width: 100%; padding: 8px 6px; border: 0; border-top: 1px solid var(--line); background: none; color: inherit; font: inherit; text-align: left; cursor: pointer; }
  .asrow:hover { background: var(--panel2); }
  .asn { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .aschips { display: flex; gap: 6px; flex-wrap: wrap; justify-content: flex-end; }
</style>
