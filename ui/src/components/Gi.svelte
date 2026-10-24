<script lang="ts">
  import { GI } from '../lib/icons';
  let { n }: { n: string } = $props();
</script>

<svg class="gi" viewBox="0 0 24 24" aria-hidden="true">{@html GI[n] || ''}</svg>
