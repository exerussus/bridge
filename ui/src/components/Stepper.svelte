<script lang="ts">
  // (3.21) Этапы доставки: пройденные — зелёные, текущий светится (с процентом), ошибка — красная на своём этапе.
  let { steps, at, pct = null, bad = false }: { steps: string[]; at: number; pct?: number | null; bad?: boolean } = $props();
  const last = $derived(steps.length - 1);
  const done = (i: number) => i < at || (i === at && at === last && !bad);
  const cur = (i: number) => i === at && !done(i);
</script>

<div class="stp" role="img" aria-label={'Этап: ' + (steps[at] || '') + (pct != null ? ' ' + pct + '%' : '') + (bad ? ', ошибка' : '')}>
  <div class="stp-r">
    {#each steps as _, i}
      <div class="stp-c">
        {#if i > 0}<span class="stp-l" class:ok={i <= at && !(bad && i === at)} class:bad={bad && i === at} style={cur(i) && pct != null && !bad ? `background:linear-gradient(90deg,rgba(93,224,142,.55),var(--cyan) ${pct}%,var(--line2) ${pct}%)` : ''}></span>{/if}
        <span class="stp-d" class:ok={done(i)} class:cur={cur(i) && !bad} class:bad={cur(i) && bad}>{done(i) ? '✓' : cur(i) && bad ? '!' : ''}</span>
      </div>
    {/each}
  </div>
  <div class="stp-r">
    {#each steps as s, i}<span class="stp-t" class:ok={done(i)} class:cur={cur(i)} class:bad={cur(i) && bad}>{s}{cur(i) && pct != null && !bad ? ' ' + pct + '%' : ''}</span>{/each}
  </div>
</div>

<style>
  .stp { display: flex; flex-direction: column; gap: 6px; width: 100%; min-width: 0; }
  .stp-r { display: flex; }
  .stp-c { flex: 1; display: flex; justify-content: center; position: relative; height: 18px; align-items: center; }
  .stp-l { position: absolute; left: -50%; right: 50%; top: 8px; height: 2px; border-radius: 1px; background: var(--line2); }
  .stp-l.ok { background: rgba(93, 224, 142, 0.55); }
  .stp-l.bad { background: rgba(255, 79, 100, 0.5); }
  .stp-d { position: relative; z-index: 1; width: 16px; height: 16px; border-radius: 50%; box-sizing: border-box; display: grid; place-items: center; font: 800 9.5px/1 var(--font); color: var(--ink, #0b0c1a); background: var(--panel); border: 2px solid var(--line2); }
  .stp-d.ok { background: var(--ok); border-color: var(--ok); box-shadow: 0 0 8px rgba(93, 224, 142, 0.6); }
  .stp-d.cur { border-color: var(--cyan); box-shadow: 0 0 0 4px rgba(56, 225, 240, 0.15), 0 0 10px rgba(56, 225, 240, 0.8); }
  .stp-d.bad { background: var(--bad); border-color: var(--bad); color: #fff; box-shadow: 0 0 0 4px rgba(255, 79, 100, 0.18), 0 0 10px rgba(255, 79, 100, 0.7); }
  .stp-t { flex: 1; text-align: center; font: 600 11px/1.2 var(--font); white-space: nowrap; color: var(--dim); overflow: hidden; text-overflow: ellipsis; }
  .stp-t.ok { color: var(--ok); opacity: 0.85; }
  .stp-t.cur { color: var(--text); font-weight: 700; }
  .stp-t.bad { color: #ff7a8a; }
</style>
