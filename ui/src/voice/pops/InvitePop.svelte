<script lang="ts">
  import { R, rosterOf, onlinePeers, ringPeer } from '../engine.svelte';
  import Av from '../../components/Av.svelte';
  const people = rosterOf(R.key);
  const list = onlinePeers().filter((p: any) => !people.some(x => x.name === p.name) && R.members.includes(p.name));
</script>

<div class="small muted" style="margin-bottom:6px">У них зазвонит: зовут в «{R.title}»</div>
{#each list as p (p.name)}
  <div class="prow"><span class="pav"><Av n={p.name} title={p.title || p.name} /></span><span class="grow">{p.title || p.name}</span><span class="small muted">{p.online ? 'на связи' : 'не на связи'}</span>
    <button class="b sm" disabled={!p.online} onclick={() => ringPeer(p.name)}>Позвать</button></div>
{:else}
  <div class="small muted">Все, кто может, уже здесь</div>
{/each}
