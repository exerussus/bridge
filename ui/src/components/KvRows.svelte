<script lang="ts">
  // Дополнительные поля записи: «поле — значение», добавить и убрать.
  import type { KV } from '../lib/records';
  let { rows = $bindable() }: { rows: KV[] } = $props();
</script>

{#each rows as r, i}
  <div class="row kvr" style="gap:6px;margin-bottom:6px">
    <input class="mono" bind:value={r.k} placeholder="поле" style="width:160px" />
    <input bind:value={r.v} placeholder="значение" style="flex:1" />
    <button class="b sm ib" type="button" title="Убрать поле" onclick={() => rows.splice(i, 1)}>✕</button>
  </div>
{/each}
<button class="b sm" type="button" onclick={() => rows.push({ k: '', v: '' })}>＋ Поле</button>
