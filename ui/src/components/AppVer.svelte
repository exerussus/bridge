<script lang="ts">
  // (3.23) Версия программы из version.json: line — строка постоянной высоты на карточке (нет файла — прочерк),
  // mini — короткая запись в строке истории или доставки.
  import { type AppInfo, appTitle } from '../lib/appver';
  let { a, mode = 'line' }: { a: AppInfo | null | undefined; mode?: 'line' | 'mini' } = $props();
</script>

{#if mode === 'line'}
  <span class="avl" title={appTitle(a)}>
    {#if a}
      <span class="avv">v{a.version}</span>
      {#if a.hash}<span class="avh">{a.hash.slice(0, 7)}</span>{/if}
      {#if a.dirty}<span class="avd">+правки</span>{/if}
      {#if a.development}<span class="avdev">dev</span>{/if}
      {#if a.profile}<span class="avp ell">· {a.profile}</span>{/if}
    {:else}<span class="avp">—</span>{/if}
  </span>
{:else if a}
  <span class="avm" title={appTitle(a)}><b>v{a.version}</b>{#if a.hash}<span class="avh">{a.hash.slice(0, 7)}</span>{/if}{#if a.dirty}<span class="avd2">+правки</span>{/if}{#if a.development}<span class="avdev2">dev</span>{/if}</span>
{/if}

<style>
  .avl { display: flex; align-items: center; gap: 6px; height: 26px; min-width: 0; overflow: hidden; white-space: nowrap; font-size: 12px; }
  .avv { display: inline-flex; align-items: center; padding: 2px 9px; border-radius: 8px; background: var(--accent-soft); color: var(--cyan); font-weight: 700; flex: none; }
  .avh { font-family: var(--mono); color: var(--muted); }
  .avd { padding: 1px 8px; border-radius: 999px; background: var(--warn-soft); color: var(--warn); font-size: 11px; font-weight: 700; flex: none; }
  .avdev { padding: 1px 8px; border-radius: 999px; border: 1px solid var(--line2); font-size: 11px; font-weight: 700; flex: none; }
  .avp { color: var(--muted); min-width: 0; }
  .avm { display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
  .avm b { color: var(--cyan); }
  .avd2 { color: var(--warn); font-weight: 700; }
  .avdev2 { font-weight: 700; }
</style>
