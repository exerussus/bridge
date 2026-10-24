<script lang="ts">
  import { SET, setSet, me, openModal } from '../../lib/state.svelte';
  import { closePop } from '../../lib/pop';
  import { goSettings } from '../../lib/nav';
  import { keyName } from '../../lib/avatar';
  import { R, V, CH, roomDef, toggleVoice, setPtt } from '../engine.svelte';
  import ChanEdit from '../ChanEdit.svelte';
  import Gi from '../../components/Gi.svelte';
  const def = roomDef(R.key);
  const own = def && def.owner === me() && CH.own.some(c => c.id === def.id);
  let ptt = $state(R.ptt);
  let voice = $state(R.voice);
</script>

<label class="ptog"><input type="checkbox" checked={SET.sfx} onchange={e => setSet('sfx', (e.target as HTMLInputElement).checked)} /><span>Звуки канала</span><em>вход, выход, микрофон, показ</em></label>
<label class="ptog"><input type="checkbox" checked={SET.joinSounds} onchange={e => setSet('joinSounds', (e.target as HTMLInputElement).checked)} /><span>Звуки входа других</span></label>
<label class="ptog"><input type="checkbox" bind:checked={ptt} onchange={() => setPtt(ptt)} /><span>Рация</span><em>{keyName(SET.pttKey)}</em></label>
<div class="psep"></div>
<button class="b sm" style="width:100%" disabled={voice} title={voice ? 'Микрофон подключён' : 'Микрофон не подключился при входе — попробовать снова'} onclick={async () => { closePop(); await toggleVoice(); }}><Gi n="mic" /> {voice ? 'Микрофон подключён' : 'Подключить микрофон'}</button>
{#if own}<button class="b sm" style="width:100%;margin-top:6px" onclick={() => { closePop(); openModal(ChanEdit, { id: def.id }); }}>Участники канала…</button>{/if}
<div class="psep"></div>
<a href="#settings" onclick={e => { e.preventDefault(); closePop(); goSettings('signals'); }}>Все сигналы и пинг →</a>
