<script lang="ts">
  // Входящее предложение обновиться: карточка в углу (поверх, ничего не сдвигает).
  import { toast, titleOf, openModal } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { md } from '../lib/util';
  import HtmlModal from '../modals/HtmlModal.svelte';
  let st = $state<any>(null);
  let hidden = $state('');
  async function poll() { try { st = await api('GET', 'update/state'); } catch {} }
  $effect(() => { const t0 = setTimeout(poll, 800), t = setInterval(poll, 4000); return () => { clearTimeout(t0); clearInterval(t); }; });
  const busy = $derived(st && ['download', 'install', 'restart'].includes(st.status));
  const o = $derived(st?.offer);
  const show = $derived(!!st && ((o && hidden !== o.version) || busy || st.status === 'error' || st.status === 'done'));
  const LINE: Record<string, string> = { offer: 'Ждёт вашего решения', download: 'Скачиваю новые файлы…', install: 'Заменяю файлы…', restart: 'Перезапускаюсь — окно откроется само', done: 'Установлено — перезапустите мост' };
  const line = $derived(!st ? '' : st.status === 'error' ? 'Не получилось: ' + st.error : st.status === 'done' ? st.error || LINE.done : LINE[st.status] || '');
  async function later() {
    if (o) hidden = o.version;
    else if (!busy) { try { await api('POST', 'update/decline'); } catch {} st = null; }
  }
  async function yes() { try { await api('POST', 'update/accept'); st.status = 'download'; } catch (e: any) { toast(e.message, 'bad'); } }
  function notes() {
    if (!o) return;
    openModal(HtmlModal, { html: '<div class="row"><h3 style="margin:0">Что нового в v' + o.version + '</h3></div><div class="changelog">' + md(o.notes) + '</div>' });
  }
</script>

<div class="card updcard" class:on={show}>
  {#if st}
    <div class="row"><b>Обновление моста</b><div class="grow"></div><span class="pill acc">v{o ? o.version : st.version}</span></div>
    <div class="small" style="margin:6px 0">{#if o}«{titleOf(o.from)}» предлагает обновить мост с v{st.version} до v{o.version}.{:else}&nbsp;{/if}</div>
    <div class="small muted updline" class:bad={st.status === 'error'}>{line}</div>
    <div class="row" style="margin-top:8px;gap:8px">
      <button class="b sm" disabled={!(o && o.notes)} onclick={notes}>Что нового</button><div class="grow"></div>
      <button class="b sm" disabled={busy} onclick={later}>{o ? 'Позже' : 'Скрыть'}</button>
      <button class="b sm pri" style="min-width:96px" disabled={!(o && !busy)} onclick={yes}>Обновить</button>
    </div>
  {/if}
</div>
