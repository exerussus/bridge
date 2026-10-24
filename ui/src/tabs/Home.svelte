<script lang="ts">
  import { ui, setTab, toast, load, peerTitle, openModal } from '../lib/state.svelte';
  import { api, act } from '../lib/api';
  import { ago, size } from '../lib/util';
  import Pill from '../components/Pill.svelte';
  import MailRow from '../components/MailRow.svelte';
  import Gi from '../components/Gi.svelte';
  import CheckPeer from '../modals/CheckPeer.svelte';

  const ov = $derived(ui.ov);
  const live = $derived(ov.peers.filter((p: any) => p.enabled && !p.invite));
  const on = $derived(live.filter((p: any) => p.online).length);
  const files = $derived(ov.shares.reduce((a: number, s: any) => a + s.files, 0));
  const recs = $derived(ov.shares.reduce((a: number, s: any) => a + s.records, 0));
  const open = $derived(ov.shares.reduce((a: number, s: any) => a + s.open, 0));
  const inbox = $derived(ov.shares.reduce((a: number, s: any) => a + s.inbox, 0));
  const pend = $derived(Object.values(ov.mailbox.pending || {}).reduce((a: number, p: any) => a + p.files, 0) as number);
  const conflicts = $derived(ov.shares.reduce((a: number, s: any) => a + s.conflicts.length, 0));
  const rejected = $derived(ov.shares.reduce((a: number, s: any) => a + s.rejected, 0));

  let recent = $state<any[]>([]);
  async function loadRecs() {
    const all: any[] = [];
    for (const s of ui.ov.shares) { try { (await api('GET', 'records?share=' + encodeURIComponent(s.id))).forEach((r: any) => all.push(r)); } catch {} }
    all.sort((a, b) => +new Date(b.updated) - +new Date(a.updated));
    recent = all.slice(0, 8);
  }
  $effect(() => { void ui.ov; loadRecs(); });

  function openRecord(share: string, id: string) { ui.recShare = share; ui.curRecord = id; setTab('records'); }

  async function decide(c: any, yes: boolean) {
    if (!yes) {
      const why = prompt('Причина отказа:', '');
      if (why === null) return;
      try { await act({ action: 'reject', share: c.share, record: c.record, note: why || 'отклонено в панели', actor: 'панель' }); toast('Отклонено'); } catch (e: any) { toast(e.message, 'bad'); }
    } else {
      if (!confirm('Выполнить команду ' + c.record + ' на этом ПК?')) return;
      try { await act({ action: 'approve', share: c.share, record: c.record, actor: 'панель', note: 'подтверждено в панели' }); toast('Подтверждено', 'ok'); } catch (e: any) { toast(e.message, 'bad'); }
    }
    load();
  }
</script>

<div class="grid">
  <div class="card" style="cursor:pointer" onclick={() => setTab('peers')} role="button" tabindex="0" onkeydown={() => {}}>
    <h3>Соседи</h3><div class="big">{on} / {live.length}</div>
    <div class="muted small">{on === live.length ? 'все на связи' : 'не на связи: ' + live.filter((p: any) => !p.online).map((p: any) => p.title || p.name).join(', ')}</div>
  </div>
  <div class="card" style="cursor:pointer" onclick={() => setTab('records')} role="button" tabindex="0" onkeydown={() => {}}>
    <h3>Записи</h3><div class="big">{recs}</div><div class="muted small">открытых {open} · ждут меня {inbox}</div>
  </div>
  <div class="card" style="cursor:pointer" onclick={() => setTab('shares')} role="button" tabindex="0" onkeydown={() => {}}>
    <h3>Файлы пространств</h3><div class="big">{files}</div><div class="muted small">{ov.shares.map((s: any) => s.id + ': ' + size(s.bytes)).join(' · ')}</div>
  </div>
  <div class="card" style="cursor:pointer" onclick={() => setTab('send')} role="button" tabindex="0" onkeydown={() => {}}>
    <h3>Ждут отправки</h3><div class="big">{pend}</div><div class="muted small">{pend ? 'сосед заберёт, как будет на связи' : 'очередь пуста'}</div>
  </div>
  {#if conflicts + rejected}
    <div class="card" style="cursor:pointer" onclick={() => setTab('shares')} role="button" tabindex="0" onkeydown={() => {}}>
      <h3>Внимание</h3><div class="big">{conflicts + rejected}</div><div class="muted small">конфликтов файлов {conflicts} · отклонённых событий {rejected}</div>
    </div>
  {/if}
</div>

<div class="card" class:hide={!ov.waiting.length} style="margin-top:14px">
  <h3>⏳ Ждёт вашего подтверждения</h3>
  {#each ov.waiting as c (c.record)}
    <div class="row" style="padding:6px 0;border-bottom:1px solid var(--line)">
      <div class="grow"><b>{c.from}</b> просит <span class="tag">{c.runner}</span> <span class="mono">{c.args}</span>{c.note ? ' — ' + c.note : ''}
        <div class="small muted">{c.record} · {ago(c.since)}</div></div>
      <button class="b ok" onclick={() => decide(c, true)}>✔ Выполнить</button>
      <button class="b danger" onclick={() => decide(c, false)}>✖ Отказать</button>
    </div>
  {/each}
</div>

<div class="split" style="margin-top:14px">
  <div class="card"><h3>Соседи</h3>
    {#if ov.peers.length}
      <table><tbody>
        {#each ov.peers as p (p.name)}
          <tr>
            <td><span class="dot {p.online ? 'ok' : p.enabled ? 'bad' : ''}"></span> <b>{p.title || p.name}</b>{#if p.title} <span class="muted small">{p.name}</span>{/if}</td>
            <td class="small muted">
              {#if p.enabled}
                {#if p.online}на связи · {ago(p.lastSeen)}{p.latency != null ? ' · ' + p.latency + ' мс' : ''}{#if p.inboundError} · <span style="color:var(--bad)">← {p.inboundError}</span>{/if}
                {:else}{p.error || 'нет связи'}{/if}
              {:else}выключен{/if}
            </td>
            <td>{#if p.enabled && (!p.online || p.inboundError || (p.issues || []).length)}<button class="b sm" title="почему нет связи или обмена" onclick={() => openModal(CheckPeer, { name: p.name })}><Gi n="pulse" /></button>{/if}</td>
          </tr>
        {/each}
      </tbody></table>
    {:else}<div class="empty">Соседей пока нет</div>{/if}
  </div>
  <div class="card"><h3>Последние передачи файлов</h3>
    {#if (ov.mailbox.recent || []).length}
      <table><tbody>{#each ov.mailbox.recent.slice(0, 8) as e}<MailRow {e} />{/each}</tbody></table>
    {:else}<div class="empty">Пока ничего не передавали</div>{/if}
  </div>
</div>

<div class="card" style="margin-top:14px"><h3>Недавно изменённые записи</h3>
  {#if recent.length}
    <table><tbody>
      {#each recent as r (r.share + r.id)}
        <tr class="click" onclick={() => openRecord(r.share, r.id)}>
          <td><b>{r.title || r.id}</b> <span class="small muted">{r.id}</span></td><td><Pill s={r.status} /></td>
          <td class="small">{r.assignee ? peerTitle(r.assignee) : ''}</td><td class="small muted">{ago(r.updated)}</td>
        </tr>
      {/each}
    </tbody></table>
  {:else}<div class="empty">Записей пока нет — поставьте первую задачу во вкладке «Записи»</div>{/if}
</div>
