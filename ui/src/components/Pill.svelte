<script lang="ts">
  import { statusOf } from '../lib/util';
  let { s }: { s: string } = $props();
  const v = $derived(statusOf(s));
</script>

{#if s}<span class="pill {v[1]}">{v[0]}</span>{/if}
