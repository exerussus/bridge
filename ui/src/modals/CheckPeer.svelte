<script lang="ts">
  import { closeModal, load, peerTitle, toast } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { copy } from '../lib/util';
  import { STEP } from '../lib/peers';
  let { name = null }: { name?: string | null } = $props();
  let res = $state<any[] | null>(null);
  let err = $state('');
  let busy = $state(false);
  async function run() {
    busy = true; err = ''; res = null;
    try {
      const r = await api('GET', 'peers/check' + (name ? '?' + new URLSearchParams({ name }) : ''));
      res = Array.isArray(r) ? r : [r];
      await load();
    } catch (e: any) { err = e.message; }
    busy = false;
  }
  run();
  function report() {
    if (!res) return;
    copy(res.map(c => (c.title || c.peer) + ' (' + c.peer + '): ' + c.summary + '\n' + c.steps.map((s: any) => '  [' + STEP[s.state][0] + '] ' + s.title + ': ' + s.text + (s.fix ? '\n      → ' + s.fix : '')).join('\n')).join('\n\n'), toast);
  }
</script>

<h3>Проверка связи{name ? ' с «' + peerTitle(name) + '»' : ''}</h3>
<div>
  {#if busy}<div class="small muted">Проверяю по шагам…</div>
  {:else if err}<div class="pill bad">{err}</div>
  {:else if res && !res.length}<div class="empty">Соседей нет</div>
  {:else if res}
    {#each res as c (c.peer)}
      <div style="margin-bottom:14px">
        <div class="row"><b>{c.title || c.peer}</b>{#if c.title} <span class="small muted mono">{c.peer}</span>{/if}<div class="grow"></div>
          <span class="pill {STEP[c.state][1]}">{c.state === 'ok' ? 'всё в порядке' : c.state === 'warn' ? 'есть замечания' : 'есть проблема'}</span></div>
        <table style="margin-top:6px"><tbody>
          {#each c.steps as s}
            <tr><td style="width:26px"><span class="pill {STEP[s.state][1]}">{STEP[s.state][0]}</span></td><td style="width:32%"><b class="small">{s.title}</b></td>
              <td class="small">{s.text}{#if s.fix}<div style="margin-top:3px;color:var(--accent)">→ {s.fix}</div>{/if}</td></tr>
          {/each}
        </tbody></table>
      </div>
    {/each}
  {/if}
</div>
<div class="row" style="margin-top:12px">
  <button class="b" onclick={report} disabled={!res}>Скопировать отчёт</button><div class="grow"></div>
  <button class="b" onclick={run} disabled={busy}>Проверить ещё раз</button><button class="b pri" onclick={closeModal}>Закрыть</button>
</div>
