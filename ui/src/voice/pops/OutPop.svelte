<script lang="ts">
  import { SET, setSet } from '../../lib/state.svelte';
  import { applyOut } from '../engine.svelte';
  import { closePop } from '../../lib/pop';
  import { goSettings } from '../../lib/nav';
  let devs = $state<MediaDeviceInfo[]>([]);
  const sinkOk = !!(HTMLMediaElement.prototype as any).setSinkId;
  (async () => { try { devs = (await navigator.mediaDevices.enumerateDevices()).filter(x => x.kind === 'audiooutput' && x.deviceId !== 'default'); } catch {} })();
</script>

<div class="plbl">Звук собеседников</div>
{#if sinkOk}
  {#each [{ deviceId: '', label: 'Как в системе' }, ...devs] as o (o.deviceId)}
    <button class="popt" class:on={(SET.out || '') === o.deviceId} onclick={() => { setSet('out', o.deviceId); applyOut(); }}><i></i>{o.label || 'динамики'}</button>
  {/each}
{/if}
<div class="psep"></div>
<div class="plbl">Общая громкость · <span>{Math.round(SET.outVol * 100)}%</span></div>
<input type="range" min="0" max="1" step="0.05" value={SET.outVol} style="width:100%" oninput={e => { setSet('outVol', +(e.target as HTMLInputElement).value); applyOut(); }} />
<div class="psep"></div>
<a href="#settings" onclick={e => { e.preventDefault(); closePop(); goSettings('audio'); }}>Все настройки звука →</a>
