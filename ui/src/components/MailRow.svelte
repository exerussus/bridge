<script lang="ts">
  import { peerTitle } from '../lib/state.svelte';
  import { ago, size, fullTime } from '../lib/util';
  let { e }: { e: any } = $props();
  const K: Record<string, [string, string]> = { sent: ['→ доставлено', 'ok'], received: ['← получено', 'acc'], error: ['ошибка', 'bad'] };
  const k = $derived(K[e.kind] || [e.kind, '']);
</script>

<tr>
  <td><span class="pill {k[1]}">{k[0]}</span></td>
  <td>{peerTitle(e.peer)}</td>
  <td class="mono">{e.path}{#if e.message} <span class="muted">{e.message}</span>{/if}</td>
  <td class="small muted" title={fullTime(e.time)}>{e.size ? size(e.size) + ' · ' : ''}{ago(e.time)}</td>
</tr>
