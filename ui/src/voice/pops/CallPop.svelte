<script lang="ts">
  import { ui, toast } from '../../lib/state.svelte';
  import { closePop } from '../../lib/pop';
  import { onlinePeers, callPeers } from '../engine.svelte';
  const peers = onlinePeers();
  const groups = Object.entries(ui.ov.groups || {}) as [string, any][];
  let picked = $state<string[]>([]);
  function group(g: string) {
    const members = (ui.ov.groups[g] || {}).members || [];
    for (const p of peers) if (members.includes(p.name) && p.online && !picked.includes(p.name)) picked.push(p.name);
  }
  async function start() {
    if (!picked.length) { toast('Отметьте, кому позвонить', 'bad'); return; }
    closePop();
    await callPeers([...picked]);
  }
</script>

<div class="small muted" style="margin-bottom:6px">Разговор без канала: у выбранных зазвонит</div>
<div class="pcol">
  {#each peers as p (p.name)}
    <label class="chip"><input type="checkbox" value={p.name} bind:group={picked} disabled={!p.online} /> {p.title || p.name}{#if !p.online} <span class="muted">(нет связи)</span>{/if}</label>
  {:else}
    <span class="small muted">Соседей пока нет — подключите во вкладке «Соседи»</span>
  {/each}
  {#if groups.length}<span class="small muted" style="margin-left:8px">группы:</span>{#each groups as [g, v]}<button class="b sm" onclick={() => group(g)}>{v.title || g}</button>{/each}{/if}
</div>
<button class="b pri" style="width:100%;margin-top:8px" onclick={start}>Позвонить</button>
