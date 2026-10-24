<script lang="ts">
  // «Поручить агенту» из записи-задачи: кому (сосед с агентом), текст поручения (из задачи, можно поправить).
  import { closeModal, toast, titleOf, ui } from '../lib/state.svelte';
  import { agentPeers, taskPrompt, delegate, openThread } from '../lib/agenttask';
  let { share, rec, onstarted }: { share: string; rec: any; onstarted?: (id: string) => void } = $props();
  let peers = $state<{ name: string; mode: string; local?: boolean }[] | null>(null);
  let peer = $state(rec.fields?.agent || '');
  let prompt = $state(taskPrompt(share, rec));
  let assign = $state(true);
  let busy = $state(false);
  $effect(() => { agentPeers().then(p => { peers = p; if (!peer || !p.some(x => x.name === peer)) peer = p[0]?.name || ''; }); });
  const mine = rec.creator === ui.ov.node.node;
  async function go() {
    if (!peer) return;
    busy = true;
    try { const id = await delegate(share, rec, peer, prompt.trim(), assign); closeModal(); onstarted?.(id); if (confirm('Открыть ход работы во вкладке «Агенты»?')) openThread(id); }
    catch (e: any) { toast(e.message, 'bad'); busy = false; }
  }
</script>

<h3>Поручить агенту · {rec.fields?.title || rec.id}</h3>
{#if peers === null}<div class="small muted">Смотрю, кто принимает поручения…</div>
{:else if !peers.length}<div class="pill warn">Поручить некому: включите агента на этом ПК («Агенты» → режим) или попросите соседа разрешить поручения от этого ПК.</div>
{:else}
  <label class="f">Кому<select bind:value={peer}>{#each peers as p}<option value={p.name}>{p.local ? 'Этот ПК' : titleOf(p.name)} · {p.mode === 'auto' ? 'Claude запускается сам' : 'ждёт сессию Claude'}</option>{/each}</select></label>
  <label class="f">Поручение<textarea rows="8" bind:value={prompt}></textarea></label>
  <label class="small"><input type="checkbox" bind:checked={assign} disabled={!mine} /> назначить исполнителем задачи ПК агента — тогда статус задачи агент ведёт сам (в работе → на проверке){mine ? '' : ' · менять исполнителя может только постановщик'}</label>
  <div class="small muted" style="margin-top:6px">«Готово» ставите вы, когда проверите. Продолжить работу — в той же нити во вкладке «Агенты».</div>
{/if}
<div class="row" style="margin-top:12px"><div class="grow"></div><button class="b" onclick={closeModal}>Отмена</button><button class="b pri" disabled={!peer || busy || !prompt.trim()} onclick={go}>Поручить</button></div>
