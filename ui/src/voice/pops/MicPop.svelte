<script lang="ts">
  import { SET, setSet } from '../../lib/state.svelte';
  import { remic, setMicGain, micLevel } from '../engine.svelte';
  import { closePop } from '../../lib/pop';
  import { goSettings } from '../../lib/nav';
  import { NS_MODES, nsMode } from '../denoise';
  let devs = $state<MediaDeviceInfo[]>([]);
  let lvl = $state(0);
  (async () => { try { devs = (await navigator.mediaDevices.enumerateDevices()).filter(x => x.kind === 'audioinput' && x.deviceId !== 'default'); } catch {} })();
  $effect(() => { const t = setInterval(() => (lvl = Math.min(100, micLevel() / 1.28)), 120); return () => clearInterval(t); });
  async function pick(id: string) { setSet('mic', id); await remic(); }
</script>

<div class="plbl">Микрофон</div>
{#each [{ deviceId: '', label: 'Как в системе' }, ...devs] as o (o.deviceId)}
  <button class="popt" class:on={(SET.mic || '') === o.deviceId} onclick={() => pick(o.deviceId)}><i></i>{o.label || 'микрофон'}</button>
{/each}
<div class="psep"></div>
<div class="plbl">Громкость · <span>{Math.round(SET.micGain * 100)}%</span></div>
<input type="range" min="0" max="2" step="0.05" value={SET.micGain} style="width:100%" oninput={e => { const v = +(e.target as HTMLInputElement).value; setMicGain(v); setSet('micGain', v); }} />
<div class="lvl2"><i style="width:{lvl}%"></i></div>
<div class="psep"></div>
<div class="plbl">Шумоподавление</div>
{#each NS_MODES as [v, t, d] (v)}
  <button class="popt" class:on={nsMode() === v} title={d} onclick={() => { setSet('nsMode', v); remic(); }}><i></i>{t}</button>
{/each}
<div class="psep"></div>
<a href="#settings" onclick={e => { e.preventDefault(); closePop(); goSettings('audio'); }}>Все настройки звука →</a>
