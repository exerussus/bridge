<script lang="ts">
  import { closeModal, toast, openModal, SET, setSet } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { md } from '../lib/util';
  import ShareVersion from './ShareVersion.svelte';
  let r = $state<any>(null);
  const prev = SET.seenVersion;
  (async () => {
    try { r = await api('GET', 'changelog'); setSet('seenVersion', r.version); } catch (e: any) { toast(e.message, 'bad'); closeModal(); }
  })();
</script>

{#if r}
  <div class="row"><h3 style="margin:0">Что нового</h3><div class="grow"></div><span class="pill acc">v{r.version}</span></div>
  {#if prev && prev !== r.version}<div class="small muted" style="margin:6px 0 0">Вы обновились с v{prev}.</div>{/if}
  <div class="changelog">{@html md(r.text)}</div>
  <div class="row" style="margin-top:12px"><button class="b" title="Предложить соседям со старой версией обновиться до этой" onclick={() => openModal(ShareVersion)}>Поделиться версией</button><div class="grow"></div><button class="b pri" onclick={closeModal}>Понятно</button></div>
{:else}
  <div class="small muted">Загружаю…</div>
{/if}
