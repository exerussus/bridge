<script lang="ts">
  // Управление ПК: красная полоса «вами управляют» и окно запроса (разрешить — удерживать 2 с, отклонить — сразу).
  import { titleOf, toast } from '../lib/state.svelte';
  import { R } from '../voice/engine.svelte';
  import { SX, revoke, askGrant, askDecline } from '../voice/screen.svelte';
  let now = $state(Date.now());
  $effect(() => { const t = setInterval(() => (now = Date.now()), 1000); return () => clearInterval(t); });
  const sec = $derived(SX.ch ? Math.max(0, Math.floor((now - SX.ch.since) / 1000)) : 0);
  const S = $derived(SX.ask ? R.mine[SX.ask.stream] : null);
  const ok = $derived(!!S && S.surf === 'monitor');
  /** Кнопка разрешения: держать 2 секунды — случайно не нажать. */
  function hold(node: HTMLButtonElement, keyboard: boolean) {
    let t: any = 0;
    const stop = () => { clearTimeout(t); node.classList.remove('holding'); };
    node.addEventListener('pointerdown', e => {
      if (e.button !== 0) return;
      node.setPointerCapture(e.pointerId); node.classList.add('holding');
      t = setTimeout(() => { stop(); askGrant(keyboard); }, 2000);
    });
    node.addEventListener('pointerup', stop); node.addEventListener('pointercancel', stop); node.addEventListener('lostpointercapture', stop);
    node.addEventListener('keydown', e => e.preventDefault()); node.addEventListener('click', e => e.preventDefault());
    return { destroy: stop };
  }
</script>

{#if SX.ch}
  <div class="ctlbar">
    <svg class="gi" viewBox="0 0 24 24"><rect x="6" y="3" width="12" height="18" rx="6" /><path d="M12 7v4" /></svg>
    <b>{titleOf(SX.ch.peer)} управляет вашим ПК</b>
    <span>{SX.ch.keyboard ? 'мышь и клавиатура' : 'только мышь'} · {String(Math.floor(sec / 60)).padStart(2, '0')}:{String(sec % 60).padStart(2, '0')} · мост заперт: права, папки и настройки не изменить</span>
    <button class="b sm" onclick={() => { revoke('остановлено'); toast('Управление остановлено', 'ok'); }}>Остановить</button>
  </div>
{/if}
{#if SX.ask}
  <div class="ctlask">
    <div class="card">
      <div class="row" style="gap:10px"><svg class="gi" viewBox="0 0 24 24"><rect x="6" y="3" width="12" height="18" rx="6" /><path d="M12 7v4" /></svg><h3 style="margin:0">Запрос управления</h3></div>
      <p><b>{titleOf(SX.ask.n)}</b> просит управлять вашим ПК: <b>{SX.ask.keyboard ? 'мышь и клавиатура' : 'мышь'}</b>.</p>
      {#if ok}
        <p class="small muted">Показ «{S.label}» ({S.w}×{S.h}). Он сможет двигать мышь и щёлкать{SX.ask.keyboard ? ', а также печатать и нажимать клавиши' : ''}. Окно моста ему недоступно, а сам мост на это время заперт: права, папки, соседей и настройки изменить нельзя. Остановить можно в любой момент — красной полосой сверху, в меню значка у часов или клавишей Pause.</p>
        <div class="holds">
          <button class="b hold" use:hold={false}><i></i><span>Удерживайте 2 с — разрешить только мышь</span></button>
          {#if SX.ask.keyboard}<button class="b hold danger" use:hold={true}><i></i><span>Удерживайте 2 с — мышь и клавиатура</span></button>{/if}
        </div>
      {:else}
        <p class="small muted">Управлять можно, только когда вы показываете весь экран (не окно и не вкладку).</p>
      {/if}
      <div class="row" style="margin-top:12px"><div class="grow"></div><button class="b" onclick={() => askDecline()}>Отклонить</button></div>
    </div>
  </div>
{/if}
