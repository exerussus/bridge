<script lang="ts">
  // Аватарка участника: картинка (или гифка) из моста, иначе — первая буква имени.
  import { aurl } from '../lib/api';
  import { avatarUrl, AV } from '../voice/engine.svelte';
  let { n, title = '' }: { n: string; title?: string } = $props();
  const url = $derived((void AV.raw, avatarUrl(n)));
</script>

{#if url}<img alt="" src={aurl(url)} />{:else}{String(title || n || '?').slice(0, 1).toUpperCase()}{/if}
