<script lang="ts">
  // Вопрос агента прямо в задаче: если по её нити агент ждёт ответа — показать и дать ответить.
  import { api } from '../lib/api';
  import AgentAsk from './AgentAsk.svelte';
  let { id }: { id: string } = $props();
  let th = $state<any>(null);
  $effect(() => {
    const tid = id; let t: any = 0, live = true;
    const tick = async () => { try { const r = await api('GET', 'agent/thread?id=' + encodeURIComponent(tid) + '&after=999999999'); if (live) th = r; } catch { if (live) th = null; } if (live) t = setTimeout(tick, 4000); };
    tick();
    return () => { live = false; clearTimeout(t); };
  });
</script>
{#if th && ['running', 'waiting', 'queued'].includes(th.status)}<AgentAsk {th} onanswered={async () => { th = await api('GET', 'agent/thread?id=' + encodeURIComponent(id) + '&after=999999999'); }} />{/if}
