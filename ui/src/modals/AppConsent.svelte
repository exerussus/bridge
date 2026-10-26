<script lang="ts">
  // Разрешение приложения: что запустится на этом ПК и какие права ему дать. Без него приложение не открывается.
  import { closeModal, toast } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { AP, PERM_LABEL, loadApps, openApp, type AppInfo } from '../lib/apps.svelte';
  let { id, then }: { id: string; then?: boolean } = $props();
  const a = $derived(AP.list.find(x => x.id === id) as AppInfo | undefined);
  // по умолчанию — всё, что приложение просит (снять галочку можно); задаём сразу, до привязки флажков
  let picked = $state<Record<string, boolean>>(Object.fromEntries((AP.list.find(x => x.id === id)?.perms || []).map(p => [p, true])));
  let busy = $state(false);
  const hue = (s: string) => { let h = 0; for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0; return h % 360; };
  const runs = $derived(!a ? [] : [
    ...(a.native ? ['своё окно'] : []),
    ...(a.service ? ['фоновая служба'] : []),
    ...(a.actions.length ? ['кнопки: ' + a.actions.map((x: any) => x.title).join(', ')] : []),
  ]);
  async function allow() {
    if (!a) return;
    busy = true;
    try {
      const go = !!then, app = id;
      await api('POST', 'apps/allow', { id: app, perms: a.perms.filter(p => picked[p]) });
      await loadApps();
      closeModal();
      if (go) openApp(app);
    } catch (e: any) { toast(e.message, 'bad'); busy = false; }
  }
  async function remove() {
    if (!a || !confirm('Убрать «' + a.name + '» в корзину моста?')) return;
    try { await api('POST', 'apps/remove', { id }); toast('Убрано в корзину моста'); closeModal(); loadApps(); } catch (e: any) { toast(e.message, 'bad'); }
  }
</script>

{#if a}
  <div class="row" style="gap:14px;align-items:center">
    <span class="appic big" style="--h:{hue(a.id)}">{#if a.icon}<img src={'/apps/' + encodeURIComponent(a.id) + '/icon'} alt="" />{:else}{a.name.slice(0, 2)}{/if}</span>
    <div><div style="font-size:20px;font-weight:600">{a.name} <span class="muted" style="font-size:14px;font-weight:400">{a.version}</span></div>
      <div class="small muted">{a.state === 'changed' ? 'Приложение изменилось — разрешите заново' : 'Новое приложение в папке «приложения»'}{a.author ? ' · ' + a.author : ''} · {a.packaged ? 'пакет .most-app' : 'папка'}</div></div>
  </div>
  {#if a.description}<p class="small" style="margin:10px 0 0">{a.description}</p>{/if}
  {#if runs.length}
    <div class="pill warn" style="display:block;margin-top:12px;line-height:1.5">Запустит на этом ПК: <b>{runs.join('; ')}</b>. Это программа из пакета — разрешайте, только если доверяете автору. Отпечаток: <span class="mono">{a.hash.slice(0, 12)}</span></div>
  {/if}
  {#if a.perms.length}
    <div class="plbl" style="margin-top:12px">Что ему можно</div>
    {#each a.perms as p (p)}
      <label class="small" style="display:flex;gap:8px;align-items:center;padding:3px 0"><input type="checkbox" bind:checked={picked[p]} /> {PERM_LABEL[p] || p}</label>
    {/each}
  {:else}<div class="small muted" style="margin-top:10px">Особых прав не просит: только свои настройки и окно.</div>{/if}
  <div class="small muted" style="margin-top:10px">Никогда: управление ПК, настройки моста, соседи и права доступа, ключи. Сменится программа или список прав — мост спросит снова.</div>
  <div class="row" style="margin-top:14px;gap:8px"><button class="b danger" onclick={remove}>Убрать</button><div class="grow"></div><button class="b" onclick={closeModal}>Не сейчас</button><button class="b pri" disabled={busy} onclick={allow}>{then ? 'Разрешить и открыть' : 'Разрешить'}</button></div>
{:else}
  <div class="empty">Приложение убрано</div>
  <div class="row"><div class="grow"></div><button class="b" onclick={closeModal}>Закрыть</button></div>
{/if}
