<script lang="ts">
  import { titleOf } from '../../lib/state.svelte';
  import { closePop } from '../../lib/pop';
  import { askControl } from '../screen.svelte';
  let { id, n }: { id: string; n: string } = $props();
</script>

<div class="plbl">Попросить управление ПК {titleOf(n)}</div>
<button class="popt" onclick={() => { closePop(); askControl(id, n, false); }}><i></i>Только мышь</button>
<button class="popt" onclick={() => { closePop(); askControl(id, n, true); }}><i></i>Мышь и клавиатура</button>
<div class="small muted">Он увидит запрос и решит сам.</div>
