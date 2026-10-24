<script lang="ts">
  import { titleOf } from '../../lib/state.svelte';
  import { R, setPeerVol } from '../engine.svelte';
  let { n }: { n: string } = $props();
  let v = $state(R.vol[n] ?? 1);
</script>

<div class="plbl">{titleOf(n)} · <span>{Math.round(v * 100)}%</span></div>
<input type="range" min="0" max="1" step="0.05" value={v} style="width:200px" oninput={e => { v = +(e.target as HTMLInputElement).value; setPeerVol(n, v); }} />
