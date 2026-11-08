<script lang="ts">
  // «Отправка» (3.21): слева — кому и куда перетащить, справа — В пути / Получено / История.
  // Файлы собраны в пакеты (папка или файл верхнего уровня, сосед, день) и раскрываются — без бесконечной прокрутки.
  // У того, что в пути, — этапы: в очередь → ждёт соседа → передача → получено.
  import { ui, toast, SET, setSet, titleOf } from '../lib/state.svelte';
  import { api, upload, download } from '../lib/api';
  import { ago, size, fullTime, pickFiles } from '../lib/util';
  import Drop from '../components/Drop.svelte';
  import Gi from '../components/Gi.svelte';
  import Stepper from '../components/Stepper.svelte';

  const peers = $derived(ui.ov.peers.filter((p: any) => p.enabled && !p.invite));
  let peer = $state('');
  $effect(() => {
    if (!peer || !peers.some((p: any) => p.name === peer)) {
      const last = SET.sendPeer;
      peer = last && peers.some((p: any) => p.name === last) ? last : peers[0]?.name || '';
    }
  });
  let sub = $state('');
  let q = $state('');
  let tab = $state<string>(SET.sendTab || 'in');
  let open = $state<string>('');
  let more = $state(1);
  type Up = { id: number; path: string; size: number; peer: string; part: number; done: boolean; err: string };
  let ups = $state<Up[]>([]);
  let upId = 0;
  let out = $state<{ peer: string; f: any }[]>([]);
  let inc = $state<any[]>([]);
  let xf = $state<any[]>([]);

  async function refresh() {
    try {
      const o = await api('GET', 'mailbox/outgoing');
      out = Object.entries(o).flatMap(([peer, files]: [string, any]) => files.map((f: any) => ({ peer, f })));
      inc = await api('GET', 'mailbox/incoming');
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  $effect(() => { void ui.ov; refresh(); });
  $effect(() => { xf = ui.ov?.transfers || []; });
  // пока что-то в пути — ход передач раз в секунду
  $effect(() => {
    if (!out.length) return;
    const t = setInterval(async () => { if (document.hidden) return; try { xf = await api('GET', 'transfers'); } catch {} }, 1000);
    return () => clearInterval(t);
  });

  function queue(list: { file: File; path: string }[]) {
    if (!peer) { toast('Сначала добавьте соседа', 'bad'); return; }
    const s = sub.trim().replace(/^\/+|\/+$/g, '');
    for (const it of list) send(peer, (s ? s + '/' : '') + it.path, it.file);
    setTab('out');
  }
  function send(peer: string, path: string, file: File) {
    const u: Up = { id: ++upId, path, size: file.size, peer, part: 0, done: false, err: '' };
    ups.unshift(u);
    const row = ups[0];
    upload('mailbox/upload?' + new URLSearchParams({ peer, path }), file, p => { row.part = p; })
      .then(() => { row.part = 1; row.done = true; setTimeout(() => { ups = ups.filter(x => x.id !== u.id); }, 1500); refresh(); })
      .catch(e => { row.err = e.message; });
  }
  async function cancel(peer: string, paths: string[]) {
    try { for (const path of paths) await api('POST', 'mailbox/cancel?' + new URLSearchParams({ peer, path })); toast('Убрано из очереди (в «_cancelled»)'); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function now(p: string) {
    try { const r = await api('POST', 'sync?peer=' + encodeURIComponent(p)); toast(r.online ? titleOf(p) + ' забирает сейчас' : titleOf(p) + ': ' + (r.error || 'нет связи'), r.online ? 'ok' : 'bad'); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function pickDir() {
    const f = await pickFiles({ multiple: true, dir: true });
    queue(f.map(file => ({ file, path: (file as any).webkitRelativePath || file.name })));
  }
  async function pickF() {
    const f = await pickFiles({ multiple: true });
    queue(f.map(file => ({ file, path: file.name })));
  }
  function setTab(t: string) { tab = t; more = 1; setSet('sendTab', t); }

  // ── пакеты ──
  type F = { p: string; s: number; full?: string };
  type Batch = { id: string; dir: 'up' | 'down'; peer: string; title: string; files: F[]; bytes: number; time: string; day: string;
    at?: number; pct?: number | null; bad?: boolean; meta: string };
  const dayOf = (t: string) => {
    const d = new Date(t), n = new Date(), y = new Date(n.getTime() - 864e5);
    if (d.toDateString() === n.toDateString()) return 'Сегодня';
    if (d.toDateString() === y.toDateString()) return 'Вчера';
    return d.toLocaleDateString('ru-RU', { day: 'numeric', month: 'long' });
  };
  const hhmm = (t: string) => (t ? new Date(t).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' }) : '');
  const top = (p: string) => p.split('/')[0];
  const nice = (t: string, p: string) => (t === '_builds' || t === '_сборки' ? 'сборка «' + (p.split('/')[1] || '') + '»' : t);
  const match = (b: Batch) => { const w = q.trim().toLowerCase(); return !w || b.title.toLowerCase().includes(w) || b.files.some(f => f.p.toLowerCase().includes(w)); };

  const outB = $derived.by(() => {
    const m = new Map<string, Batch>();
    for (const u of ups) {
      const k = 'u:' + u.peer + '/' + top(u.path);
      const b = m.get(k) || { id: k, dir: 'up', peer: u.peer, title: top(u.path), files: [], bytes: 0, time: new Date().toISOString(), day: 'В пути', at: 0, pct: 0, meta: '' } as Batch;
      b.files.push({ p: u.path, s: u.size }); b.bytes += u.size;
      b.pct = Math.round((b.files.reduce((a, f, i) => a + (ups.find(x => x.path === f.p)?.part || 0) * f.s, 0) / Math.max(1, b.bytes)) * 100);
      if (u.err) { b.bad = true; b.meta = u.err; } else b.meta = '→ ' + titleOf(u.peer) + ' · кладу в очередь';
      m.set(k, b);
    }
    for (const { peer, f } of out) {
      const k = 'o:' + peer + '/' + top(f.path);
      if (m.has('u:' + peer + '/' + top(f.path))) continue;
      const b = m.get(k) || { id: k, dir: 'up', peer, title: nice(top(f.path), f.path), files: [], bytes: 0, time: f.modified || '', day: 'В пути', meta: '' } as Batch;
      b.files.push({ p: f.path, s: f.size }); b.bytes += f.size;
      if ((f.modified || '') > b.time) b.time = f.modified;
      m.set(k, b);
    }
    const errs = (ui.ov.mailbox.recent || []).filter((e: any) => e.kind === 'error');
    for (const b of m.values()) {
      if (b.id.startsWith('u:')) continue;
      const p = ui.ov.peers.find((x: any) => x.name === b.peer);
      const live = xf.filter(x => x.kind === 'serve' && x.peer === b.peer && b.files.some(f => f.p === x.path));
      const err = errs.find((e: any) => e.peer === b.peer && b.files.some(f => f.p === e.path));
      if (live.length) {
        const done = live.reduce((a, x) => a + x.done, 0), tot = live.reduce((a, x) => a + (x.total || 0), 0), sp = live.reduce((a, x) => a + x.speed, 0);
        b.at = 2; b.pct = tot ? Math.round((done * 100) / tot) : null;
        b.meta = '→ ' + titleOf(b.peer) + ' · забирает · ' + size(sp) + '/с' + (live[0].eta >= 0 ? ' · осталось ' + live[0].eta + ' с' : '');
      } else if (err) { b.at = 2; b.bad = true; b.meta = '→ ' + titleOf(b.peer) + ' · ' + err.message; }
      else { b.at = 1; b.pct = null; b.meta = '→ ' + titleOf(b.peer) + (p?.online ? ' · на связи, ещё не забрал' : ' · нет связи — заберёт, как появится'); }
    }
    return [...m.values()].filter(match).sort((a, b) => (a.id < b.id ? -1 : 1));
  });
  const inB = $derived.by(() => {
    const m = new Map<string, Batch>();
    for (const f of inc) {
      const parts = f.path.split('/'), pr = parts[0], rest = parts.slice(1).join('/'), d = dayOf(f.time);
      const k = 'i:' + d + '/' + pr + '/' + top(rest);
      const b = m.get(k) || { id: k, dir: 'down', peer: pr, title: nice(top(rest), rest), files: [], bytes: 0, time: f.time, day: d, meta: '' } as Batch;
      b.files.push({ p: rest, s: f.size, full: f.path }); b.bytes += f.size;
      if (f.time > b.time) b.time = f.time;
      b.meta = 'от ' + titleOf(pr) + ' · в inbox/' + pr + '/' + top(rest);
      m.set(k, b);
    }
    return [...m.values()].filter(match).sort((a, b) => (a.time < b.time ? 1 : -1));
  });
  const histB = $derived.by(() => {
    const L: Batch[] = [];
    for (const e of ui.ov.mailbox.recent || []) {
      if (e.kind !== 'sent' && e.kind !== 'received') continue;
      const dir = e.kind === 'sent' ? 'up' : 'down', t = top(e.path), d = dayOf(e.time), last = L[L.length - 1];
      if (last && last.dir === dir && last.peer === e.peer && last.day === d && top(last.files[0].p) === t) { last.files.push({ p: e.path, s: e.size }); last.bytes += e.size; continue; }
      L.push({ id: 'h:' + L.length, dir, peer: e.peer, title: nice(t, e.path), files: [{ p: e.path, s: e.size }], bytes: e.size, time: e.time, day: d, meta: dir === 'up' ? titleOf(e.peer) + ' забрал и подтвердил' : 'получено от ' + titleOf(e.peer) } as Batch);
    }
    return L.filter(match);
  });
  const list = $derived(tab === 'out' ? outB : tab === 'in' ? inB : histB);
  const PAGE = 30;
  const groups = $derived.by(() => {
    const g: { day: string; items: Batch[] }[] = [];
    for (const b of list.slice(0, PAGE * more)) { const d = tab === 'out' ? 'В пути' : b.day; const last = g[g.length - 1]; if (last && last.day === d) last.items.push(b); else g.push({ day: d, items: [b] }); }
    return g;
  });
  const rel = (p: string) => (p.includes('/') ? p.slice(p.indexOf('/') + 1) : p);
  const STEPS = ['В очередь', 'Ждёт соседа', 'Передача', 'Получено'];
  const dl = (b: Batch, f: F) => download('mailbox/incoming/file?path=' + encodeURIComponent(f.full || b.peer + '/' + f.p), f.p.split('/').pop()).catch(er => toast(er.message, 'bad'));
  const nOut = $derived(outB.length), nIn = $derived(inB.length);
  const chev = '<path d="M6 9l6 6 6-6"/>';
</script>

<div class="sendg">
  <section class="card scomp">
    <h3 style="margin:0">Кому</h3>
    <div class="tos">
      {#each peers as p (p.name)}
        <button class="to" class:on={peer === p.name} onclick={() => { peer = p.name; setSet('sendPeer', p.name); }}>
          <span class="av2" style="width:34px;height:34px;background:var(--panel2)">{(p.title || p.name).slice(0, 1).toUpperCase()}</span>
          <span class="grow" style="min-width:0"><span class="ell" style="display:block">{p.title || p.name}</span><span class="small muted ell" style="display:block;font-weight:500">{p.online ? 'на связи · заберёт сразу' : 'нет связи · заберёт позже'}</span></span>
          <span class="dot {p.online ? 'ok' : ''}"></span>
        </button>
      {:else}<div class="small muted">Соседей нет — добавьте во вкладке «Соседи».</div>{/each}
    </div>
    <Drop onfiles={queue}>
      <div class="sdrop">
        <span class="av2" style="width:46px;height:46px;border-radius:15px;background:var(--ok-soft);color:var(--ok)"><Gi n="up" /></span>
        <b>Перетащите файлы или папки</b>
        <span class="small">для {titleOf(peer) || '…'} · до 1 ГБ на файл (maxFileMb)</span>
      </div>
    </Drop>
    <div class="row" style="gap:8px;justify-content:center;margin-top:-6px"><button class="b sm" onclick={pickF}>Файлы…</button><button class="b sm" onclick={pickDir}><Gi n="folder" /> Папку…</button></div>
    <details>
      <summary class="small muted" style="cursor:pointer">Дополнительно ›</summary>
      <label class="f" style="margin:10px 0 0">Во вложенную папку<input bind:value={sub} placeholder="необязательно, например builds/test" /></label>
      <div class="small muted" style="margin-top:6px">Сосед заберёт в «inbox/{ui.ov.node.node}/…», подтвердит — и здесь они исчезнут из «В пути».</div>
    </details>
  </section>

  <section class="card sfeed">
    <div class="row" style="gap:10px;flex-wrap:wrap;padding:0 2px 10px">
      <div class="seg2" role="tablist" aria-label="Списки">
        <button role="tab" class:on={tab === 'out'} onclick={() => setTab('out')}>В пути<span class="cnt2">{nOut}</span></button>
        <button role="tab" class:on={tab === 'in'} onclick={() => setTab('in')}>Получено<span class="cnt2">{nIn}</span></button>
        <button role="tab" class:on={tab === 'hist'} onclick={() => setTab('hist')}>История</button>
      </div>
      <div class="grow"></div>
      <input bind:value={q} placeholder="Поиск файла" aria-label="Поиск файла" style="width:190px" />
      <button class="b sm" data-open={tab === 'out' ? 'outgoing' : 'incoming'}><Gi n="folder" /> Папка</button>
    </div>
    {#each groups as g (g.day)}
      <h3 class="sday">{g.day}</h3>
      {#each g.items as b (b.id)}
        {@const o = open === b.id}
        <div class="sb2" class:o>
          <button class="sbat" onclick={() => (open = o ? '' : b.id)} aria-expanded={o}>
            <span class="av2" style={b.dir === 'up' ? 'background:var(--ok-soft);color:var(--ok)' : 'background:var(--accent-soft);color:var(--cyan)'}>{b.dir === 'up' ? '↑' : '↓'}</span>
            <span class="grow" style="min-width:0"><span class="ell" style="display:block;font-weight:700">{b.title}{#if b.files.length > 1}<span class="muted" style="font-weight:500">&nbsp;· {b.files.length} файл.</span>{/if}</span><span class="small muted ell" style="display:block" class:sbad={b.bad}>{b.meta}</span></span>
            <span class="mono small muted" style="width:72px;text-align:right">{size(b.bytes)}</span>
            <span class="small muted" style="width:48px;text-align:right" title={fullTime(b.time)}>{hhmm(b.time)}</span>
            <svg class="chev" class:up={o} viewBox="0 0 24 24" aria-hidden="true">{@html chev}</svg>
          </button>
          {#if b.at != null}<div class="sstp"><Stepper steps={STEPS} at={b.at} pct={b.pct ?? null} bad={!!b.bad} /></div>{/if}
          {#if o}
            <div class="sfiles">
              {#each [...b.files].sort((x, y) => x.p.localeCompare(y.p, 'ru', { numeric: true })).slice(0, 50) as f (f.p)}
                <div class="row small" style="gap:10px;padding:4px 0">
                  {#if b.dir === 'down' && tab === 'in'}<a class="mono ell grow" href="#dl" title={f.p} onclick={e => { e.preventDefault(); dl(b, f); }}>{rel(f.p)}</a>{:else}<span class="mono ell grow" title={f.p}>{rel(f.p)}</span>{/if}
                  <span class="mono muted">{size(f.s)}</span>
                </div>
              {/each}
              <div class="row" style="gap:8px;margin-top:8px;flex-wrap:wrap">
                <span class="small muted grow">{b.files.length > 50 ? 'и ещё ' + (b.files.length - 50) + ' файл.' : ''}</span>
                {#if tab === 'out' && !b.id.startsWith('u:')}
                  <button class="b sm" title="Попросить забрать прямо сейчас" onclick={() => now(b.peer)}>⟳ Отправить сейчас</button>
                  <button class="b sm danger" onclick={() => cancel(b.peer, b.files.map(f => f.p))}>Убрать из очереди</button>
                {:else if tab === 'in'}
                  <button class="b sm" data-open="incoming"><Gi n="folder" /> Открыть папку</button>
                {/if}
              </div>
            </div>
          {/if}
        </div>
      {/each}
    {:else}
      <div class="empty">{tab === 'out' ? 'Ничего не в пути' : tab === 'in' ? 'Пока ничего не пришло' : 'История пуста'}</div>
    {/each}
    {#if list.length > PAGE * more}<div style="text-align:center;padding:10px 0 4px"><button class="b sm" onclick={() => more++}>Показать раньше · ещё {list.length - PAGE * more}</button></div>{/if}
  </section>
</div>

<style>
  .sendg { display: flex; gap: 16px; flex-wrap: wrap; align-items: flex-start; }
  .scomp { flex: 0 1 380px; min-width: 290px; display: flex; flex-direction: column; gap: 14px; }
  .sfeed { flex: 1 1 520px; min-width: 0; padding: 16px 14px 10px; }
  .tos { display: flex; flex-direction: column; gap: 8px; }
  .to { display: flex; align-items: center; gap: 10px; padding: 8px 12px 8px 8px; border-radius: 12px; border: 1px solid var(--line2); background: transparent; color: var(--text); font: 600 13px var(--font); cursor: pointer; text-align: left; min-width: 0; }
  .to:hover { border-color: rgba(93, 224, 142, 0.5); }
  .to.on { border-color: var(--ok); background: var(--ok-soft); box-shadow: 0 0 0 1px var(--ok) inset; }
  .sdrop { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 10px 0; }
  .sday { margin: 12px 12px 6px !important; }
  .sb2 { border-radius: 12px; }
  .sb2.o { background: var(--panel2); }
  .sbat { display: flex; align-items: center; gap: 14px; width: 100%; border: 0; background: none; color: inherit; font: inherit; text-align: left; padding: 12px; border-radius: 12px; cursor: pointer; }
  .sbat:hover { background: rgba(160, 170, 255, 0.04); }
  .sbad { color: #ff7a8a !important; }
  .sstp { padding: 0 14px 12px 62px; }
  .sfiles { padding: 0 16px 12px 62px; }
</style>
