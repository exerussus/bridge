<script lang="ts">
  // (3.21) Окошко передач: перетаскивается за ручку (положение запоминается), сворачивается в одну строку.
  import { ui, SET, setSet, titleOf } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { size, dur } from '../lib/util';

  let xfers = $state<any[]>([]);
  $effect(() => { xfers = ui.ov?.transfers || []; });
  $effect(() => {
    if (!xfers.length) return;
    const t = setTimeout(async () => { if (document.hidden) return; try { xfers = await api('GET', 'transfers'); } catch {} }, 1000);
    return () => clearTimeout(t);
  });
  const XK: Record<string, string> = { up: '↑', down: '↓', serve: '↑', recv: '↓' };
  const who = (x: any) => (x.kind === 'up' ? 'отправляю ' + titleOf(x.peer) : x.kind === 'down' ? 'скачиваю у ' + titleOf(x.peer) : x.kind === 'serve' ? titleOf(x.peer) + ' забирает' : titleOf(x.peer) + ' присылает');
  const pct = (x: any) => (x.total > 0 ? Math.min(100, Math.round((x.done * 100) / x.total)) : null);
  const short = $derived(xfers.map(x => (XK[x.kind] || '⇅') + ' ' + (pct(x) ?? '…') + '%').join(' · '));

  // положение — отступ от правого нижнего угла окна
  let pos = $state({ r: SET.xfR ?? 16, b: SET.xfB ?? 16 });
  let drag: { x: number; y: number; r: number; b: number } | null = null;
  let box: HTMLDivElement;
  const clamp = (v: number, max: number) => Math.max(8, Math.min(max, v));
  function down(e: PointerEvent) {
    if ((e.target as HTMLElement).closest('button')) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag = { x: e.clientX, y: e.clientY, r: pos.r, b: pos.b };
  }
  function move(e: PointerEvent) {
    if (!drag) return;
    const w = box?.offsetWidth || 400, h = box?.offsetHeight || 60;
    pos = { r: clamp(drag.r - (e.clientX - drag.x), innerWidth - w - 8), b: clamp(drag.b - (e.clientY - drag.y), innerHeight - h - 8) };
  }
  function up() { if (!drag) return; drag = null; setSet('xfR', Math.round(pos.r)); setSet('xfB', Math.round(pos.b)); }
  // окно уменьшили — окошко не должно уехать за край
  function fit() { const w = box?.offsetWidth || 400, h = box?.offsetHeight || 60; pos = { r: clamp(pos.r, innerWidth - w - 8), b: clamp(pos.b, innerHeight - h - 8) }; }
</script>

<svelte:window onresize={fit} />

{#if xfers.length}
  <div id="xfers" bind:this={box} style="right:{pos.r}px;bottom:{pos.b}px">
    <div class="xf-h" role="presentation" onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up} title="Перетащите, чтобы передвинуть">
      <svg class="xf-grip" viewBox="0 0 24 24" aria-hidden="true"><circle cx="9" cy="6" r="1.6" /><circle cx="15" cy="6" r="1.6" /><circle cx="9" cy="12" r="1.6" /><circle cx="15" cy="12" r="1.6" /><circle cx="9" cy="18" r="1.6" /><circle cx="15" cy="18" r="1.6" /></svg>
      <b class="small">Передачи · {xfers.length}</b>
      <span class="grow"></span>
      {#if SET.xfFold}<span class="mono small xf-short ell">{short}</span>{/if}
      <button class="xf-fold" aria-label={SET.xfFold ? 'Развернуть' : 'Свернуть'} onclick={() => setSet('xfFold', !SET.xfFold)}><svg viewBox="0 0 24 24" aria-hidden="true" style:transform={SET.xfFold ? 'rotate(180deg)' : ''}><path d="M6 9l6 6 6-6" /></svg></button>
    </div>
    {#if !SET.xfFold}
      {#each xfers as x}
        {@const p = pct(x)}
        <div class="x">
          <div class="row small"><span>{XK[x.kind] || '⇅'}</span><b class="grow ell" title={x.path}>{x.path.split('/').pop()}</b><span class="muted">{p == null ? '' : p + '%'}</span></div>
          <div class="bar"><i style="width:{p == null ? 100 : p}%{p == null ? ';opacity:.4' : ''}"></i></div>
          <div class="small muted">{who(x)} · {x.what} · {size(x.done)}{x.total ? ' из ' + size(x.total) : ''} · {size(x.speed)}/с{x.eta >= 0 ? ' · осталось ' + dur(x.eta) : ''}</div>
        </div>
      {/each}
    {/if}
  </div>
{/if}
