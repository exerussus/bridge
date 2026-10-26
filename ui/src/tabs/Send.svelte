<script lang="ts">
  import { ui, toast, peerTitle, SET, setSet } from '../lib/state.svelte';
  import { api, upload, download } from '../lib/api';
  import { ago, size, fullTime, pickFiles } from '../lib/util';
  import Drop from '../components/Drop.svelte';
  import MailRow from '../components/MailRow.svelte';
  import Gi from '../components/Gi.svelte';
  import Builds from '../components/Builds.svelte';

  const peers = $derived(ui.ov.peers.filter((p: any) => p.enabled));
  let peer = $state('');
  $effect(() => {
    if (!peer || !peers.some((p: any) => p.name === peer)) {
      const last = SET.sendPeer;
      peer = last && peers.some((p: any) => p.name === last) ? last : peers[0]?.name || '';
    }
  });
  let sub = $state('');
  let tab = $state(SET.sendTab === 'builds' ? 'builds' : 'files');
  function setTab(t: string) { tab = t; setSet('sendTab', t); }
  let filter = $state('');
  type Up = { id: number; path: string; size: number; peer: string; part: number; done: boolean; err: string };
  let ups = $state<Up[]>([]);
  let upId = 0;
  let out = $state<{ peer: string; f: any }[]>([]);
  let inc = $state<any[]>([]);

  async function refresh() {
    try {
      const o = await api('GET', 'mailbox/outgoing');
      out = Object.entries(o).flatMap(([peer, files]: [string, any]) => files.map((f: any) => ({ peer, f })));
      inc = await api('GET', 'mailbox/incoming');
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  $effect(() => { void ui.ov; refresh(); });

  function queue(list: { file: File; path: string }[]) {
    if (!peer) { toast('Сначала добавьте соседа', 'bad'); return; }
    const s = sub.trim().replace(/^\/+|\/+$/g, '');
    for (const it of list) send(peer, (s ? s + '/' : '') + it.path, it.file);
  }
  function send(peer: string, path: string, file: File) {
    const u: Up = { id: ++upId, path, size: file.size, peer, part: 0, done: false, err: '' };
    ups.unshift(u);
    const row = ups[0];
    upload('mailbox/upload?' + new URLSearchParams({ peer, path }), file, p => { row.part = p; })
      .then(() => { row.part = 1; row.done = true; setTimeout(() => { ups = ups.filter(x => x.id !== u.id); }, 6000); refresh(); })
      .catch(e => { row.err = e.message; });
  }
  async function cancel(peer: string, path: string) {
    try { await api('POST', 'mailbox/cancel?' + new URLSearchParams({ peer, path })); toast('Убрано из очереди'); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function pickDir() {
    const f = await pickFiles({ multiple: true, dir: true });
    queue(f.map(file => ({ file, path: (file as any).webkitRelativePath || file.name })));
  }
  const shown = $derived(inc.filter(f => !filter || f.path.toLowerCase().includes(filter.toLowerCase())));
</script>

<div class="vtabs sttabs" role="tablist" aria-label="Отправка" style="margin-bottom:14px">
  <button role="tab" class:on={tab === 'files'} onclick={() => setTab('files')}><Gi n="up" /> Файлы</button>
  <button role="tab" class:on={tab === 'builds'} onclick={() => setTab('builds')}><Gi n="folder" /> Сборки</button>
</div>
{#if tab === 'builds'}
<Builds />
{:else}
<div class="card">
  <div class="row"><h3 style="margin:0">Отправить файлы</h3><div class="grow"></div>
    <label class="small muted">Кому&nbsp;<select bind:value={peer} onchange={() => setSet('sendPeer', peer)}>
      {#each peers as p (p.name)}<option value={p.name}>{(p.title || p.name) + (p.online ? '' : ' (нет связи)')}</option>{/each}
    </select></label>
    <label class="small muted">Во вложенную папку&nbsp;<input bind:value={sub} placeholder="необязательно" style="width:180px" /></label>
  </div>
  <Drop onfiles={queue} style="margin-top:10px">
    <div style="font-size:22px"><Gi n="up" /></div>
    <div><b>Перетащите файлы или папки сюда</b> — или нажмите, чтобы выбрать</div>
    <div class="small">Сосед заберёт их в «приём/{ui.ov.node.node}/…», подтвердит — и здесь они исчезнут из очереди.</div>
  </Drop>
  <div class="row small" style="margin-top:8px"><button class="b sm" onclick={pickDir}><Gi n="folder" /> Выбрать папку целиком</button><span class="muted">Большие файлы идут частями по сети без ограничений браузера; до 1 ГБ на файл (настройка maxFileMb).</span></div>
  <div style="margin-top:10px">
    {#each ups as u (u.id)}
      <div style="margin:6px 0">
        <div class="row small"><span class="mono grow">{u.path}</span><span class="muted">{size(u.size)} → {peerTitle(u.peer)}{u.done ? ' · в очереди' : ''}</span></div>
        {#if u.err}<div class="small" style="color:var(--bad)">{u.err}</div>{:else}<div class="bar"><i style="width:{u.part * 100}%"></i></div>{/if}
      </div>
    {/each}
  </div>
</div>
<div class="split" style="margin-top:14px">
  <div class="card"><div class="row"><h3 style="margin:0">Ждут отправки</h3><div class="grow"></div><button class="b sm" data-open="outgoing"><Gi n="folder" /> Папка</button></div>
    <div style="margin-top:8px">
      {#if out.length}
        <table><tbody>{#each out as { peer, f } (peer + f.path)}
          <tr><td>{peerTitle(peer)}</td><td class="mono">{f.path}</td><td class="small muted">{size(f.size)}</td>
            <td><button class="b sm danger" title="убрать из очереди (в «_отменено»)" onclick={() => cancel(peer, f.path)}>✖</button></td></tr>
        {/each}</tbody></table>
      {:else}<div class="empty">Очередь пуста</div>{/if}
    </div>
  </div>
  <div class="card"><div class="row"><h3 style="margin:0">Получено</h3><div class="grow"></div><input bind:value={filter} placeholder="поиск" style="width:140px" /> <button class="b sm" data-open="incoming"><Gi n="folder" /> Папка</button></div>
    <div style="margin-top:8px">
      {#if shown.length}
        <table><tbody>{#each shown.slice(0, 200) as f (f.path)}
          <tr><td class="mono"><a href="#dl" onclick={e => { e.preventDefault(); download('mailbox/incoming/file?path=' + encodeURIComponent(f.path), f.path.split('/').pop()).catch(er => toast(er.message, 'bad')); }}>{f.path}</a></td>
            <td class="small muted">{size(f.size)}</td><td class="small muted" title={fullTime(f.time)}>{ago(f.time)}</td></tr>
        {/each}</tbody></table>
      {:else}<div class="empty">Пока ничего не пришло</div>{/if}
    </div>
  </div>
</div>
<div class="card" style="margin-top:14px"><h3>История</h3>
  {#if (ui.ov.mailbox.recent || []).length}<table><tbody>{#each ui.ov.mailbox.recent as e}<MailRow {e} />{/each}</tbody></table>{:else}<div class="empty">Пусто</div>{/if}
</div>
{/if}
