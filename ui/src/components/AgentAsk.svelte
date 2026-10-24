<script lang="ts">
  // Вопрос агента людям: кнопки-варианты и/или поле ответа. Первый ответ (здесь или у заказчика) уходит агенту.
  import { api } from '../lib/api';
  import { toast, titleOf } from '../lib/state.svelte';
  import { ago } from '../lib/util';
  let { th, onanswered }: { th: any; onanswered?: () => void } = $props();
  const open = $derived((th?.requests || []).filter((r: any) => !r.answered).slice(-1)[0]);
  let text = $state('');
  let busy = $state(false);
  async function send(answer: string) {
    if (!answer.trim() || busy) return;
    busy = true;
    try { await api('POST', 'agent/answer', { thread: th.id, req: open.id, answer: answer.trim() }); text = ''; toast('Ответ ушёл агенту', 'ok'); onanswered?.(); }
    catch (e: any) { toast(e.message, 'bad'); }
    busy = false;
  }
</script>

{#if open}
  <div class="agask" role="region" aria-label="Агент спрашивает">
    <div class="row"><b>❓ Агент спрашивает</b><span class="small muted">{th.role === 'out' ? (th.local ? 'на этом ПК' : 'на ' + titleOf(th.peer)) : th.local ? 'на этом ПК' : 'по поручению ' + titleOf(th.peer)} · {ago(open.at)}</span></div>
    <div class="agq">{open.q}</div>
    {#if open.buttons.length}<div class="row agbtns">{#each open.buttons as b}<button class="b sm pri" disabled={busy} onclick={() => send(b)}>{b}</button>{/each}</div>{/if}
    {#if open.input}
      <div class="row" style="gap:6px"><input class="grow" bind:value={text} placeholder="Ответ агенту…" maxlength="4000" onkeydown={e => { if (e.key === 'Enter') send(text); }} aria-label="Ответ агенту" /><button class="b sm" disabled={busy || !text.trim()} onclick={() => send(text)}>Ответить</button></div>
    {/if}
  </div>
{/if}
