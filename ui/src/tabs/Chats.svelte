<script lang="ts">
  import { titleOf, me } from '../lib/state.svelte';
  import { CHT } from '../lib/chat.svelte';
  import ChatBox from '../components/ChatBox.svelte';
  import Av from '../components/Av.svelte';
  const ch = $derived(CHT.list.filter(c => c.kind === 'channel'));
  const dm = $derived(CHT.list.filter(c => c.kind === 'dm'));
  const cur = $derived(CHT.list.find(x => x.conv === CHT.cur));
  function pick(c: any) { CHT.cur = c.conv; setTimeout(() => (document.querySelector('#t-chat .chatbox textarea') as HTMLElement | null)?.focus(), 30); }
</script>

{#snippet item(c: any)}
  <button class="ci" class:on={CHT.cur === c.conv} onclick={() => pick(c)}>
    <span class="cav">{#if c.kind === 'channel'}#{:else}<Av n={c.peer} title={c.title} />{/if}</span>
    <span class="grow" style="min-width:0">
      <div class="nm">{#if c.kind === 'dm'}<i class="dot" class:ok={c.online} style="display:inline-block;width:7px;height:7px;border-radius:50%;margin-right:6px;background:{c.online ? 'var(--green)' : 'var(--dim)'}"></i>{/if}{c.title}</div>
      <div class="lm">{c.last && c.last.text ? (c.last.mine ? 'вы: ' : '') + c.last.text : c.kind === 'channel' ? (c.owner === me() ? 'ваш канал' : 'канал ' + titleOf(c.owner)) : 'нет сообщений'}</div>
    </span>
    {#if c.unread}<span class="cnt">{c.unread}</span>{/if}
  </button>
{/snippet}

<div class="chatgrid">
  <div class="card clist">
    <div class="grp2">Каналы</div>
    {#each ch as c (c.conv)}{@render item(c)}{:else}<div class="small muted" style="padding:4px">каналов нет — их создают во вкладке «Каналы»</div>{/each}
    <div class="grp2">Соседи</div>
    {#each dm as c (c.conv)}{@render item(c)}{:else}<div class="small muted" style="padding:4px">соседей нет</div>{/each}
  </div>
  <div class="card">
    <div class="row" style="margin-bottom:8px"><h3 style="margin:0">{cur ? (cur.kind === 'channel' ? '# ' : '') + cur.title : 'Выберите беседу'}</h3><div class="grow"></div>
      <span class="small muted">{cur ? (cur.kind === 'dm' ? (cur.online ? 'на связи' : 'не на связи — сообщения дойдут, когда появится') : 'видят все участники канала') : ''}</span></div>
    {#if cur}
      {#key cur.conv}<ChatBox conv={cur.conv} />{/key}
    {:else}
      <div class="chatbox"><div class="empty">Слева — каналы и соседи. Сообщения хранятся сутки.</div></div>
    {/if}
  </div>
</div>
