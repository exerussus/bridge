<script lang="ts">
  // Переключатель-сегменты для настройки (SET[k]); числа хранятся числами.
  import { SET, setSet } from '../lib/state.svelte';
  let { k, opts, onchange }: { k: string; opts: [string | number, string][]; onchange?: (v: any) => void } = $props();
</script>

<div class="seg">
  {#each opts as [v, t]}
    <button class:on={String(SET[k]) === String(v)} onclick={() => { setSet(k, v); onchange && onchange(v); }}>{t}</button>
  {/each}
</div>
