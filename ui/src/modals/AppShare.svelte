<script lang="ts">
  // «Отправить соседям»: у соседа появится предложение; приложение скачается, только если он примет, и запустится — только с его разрешения.
  import { closeModal, toast, ui, titleOf } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { AP, type AppInfo } from '../lib/apps.svelte';
  let { id }: { id: string } = $props();
  const a = $derived(AP.list.find(x => x.id === id) as AppInfo | undefined);
  const list = ui.ov.peers.filter((p: any) => p.enabled && !p.invite);
  let picked = $state<string[]>(list.filter((p: any) => p.online).map((p: any) => p.name));
  let res = $state<Record<string, string>>({});
  let busy = $state(false);
  let size = $state(0);
  async function go() {
    if (!picked.length) return;
    busy = true;
    try { const r = await api('POST', 'apps/share', { id, peers: picked }); res = r.results || {}; size = r.size || 0; toast('Предложение отправлено', 'ok'); } catch (e: any) { toast(e.message, 'bad'); }
    busy = false;
  }
  const mb = (n: number) => n >= 1 << 20 ? (n / (1 << 20)).toFixed(n >= 100 << 20 ? 0 : 1) + ' МБ' : Math.max(1, Math.round(n / 1024)) + ' КБ';
</script>

{#if a}
  <div class="row"><h3 style="margin:0">Отправить «{a.name}» соседям</h3><div class="grow"></div><span class="pill acc">{a.version}</span></div>
  <div class="small muted" style="margin:6px 0 10px">Соседу придёт предложение. Скачается приложение, только если он нажмёт «Принять», а запустится — только после его разрешения, как любое новое. {a.packaged ? 'Уйдёт пакет как есть.' : 'Папка будет упакована в .most-app (без .git, node_modules и отладочных папок Unity).'}</div>
  <table class="tbl upd"><tbody>
    {#each list as p (p.name)}
      <tr><td style="width:28px"><input type="checkbox" value={p.name} bind:group={picked} disabled={!p.online} /></td><td>{titleOf(p.name)}</td>
        <td class="small updres" class:muted={!res[p.name]}>{res[p.name] || (p.online ? 'на связи' : 'не на связи')}</td></tr>
    {:else}<tr><td class="muted">Соседей нет</td></tr>{/each}
  </tbody></table>
  <div class="row" style="margin-top:12px"><span class="small muted" style="min-width:120px">{size ? 'пакет ' + mb(size) : ''}</span><div class="grow"></div><button class="b" onclick={closeModal}>Закрыть</button><button class="b pri" style="min-width:150px" disabled={!picked.length || busy} onclick={go}>{busy ? 'Упаковываю…' : 'Предложить'}</button></div>
{:else}
  <div class="empty">Приложение убрано</div>
  <div class="row"><div class="grow"></div><button class="b" onclick={closeModal}>Закрыть</button></div>
{/if}
