<script lang="ts">
  import { ui, toast, peerTitle, openModal, badgeCount, SET, setSet } from '../lib/state.svelte';
  import { api, upload, download } from '../lib/api';
  import { ago, size, fullTime } from '../lib/util';
  import { igChange } from '../lib/ignore';
  import Gi from '../components/Gi.svelte';
  import Drop from '../components/Drop.svelte';
  import FolderEdit from '../modals/FolderEdit.svelte';
  import IgnoreEdit from '../modals/IgnoreEdit.svelte';

  const ACCESS: Record<string, string> = { read: 'чтение', write: 'запись' };
  let FD = $state<any>(null);
  let IG = $state<any>(null);
  let trash = $state<any[]>([]);
  let cfg = $state({ trashMin: '', up: '', down: '', max: '' });
  let cfgTouched = false;

  async function refresh() {
    try { FD = await api('GET', 'folders'); } catch (e: any) { toast(e.message, 'bad'); return; }
    if (!cfgTouched) cfg = { trashMin: String(FD.trashMinutes), up: FD.upMbit > 0 ? String(FD.upMbit) : '', down: FD.downMbit > 0 ? String(FD.downMbit) : '', max: FD.maxFileMb > 0 ? String(FD.maxFileMb) : '' };
    badgeCount('folders', FD.mounts.filter((m: any) => m.hold).length);
    if (fsCur) fsList();
    try { const t = await api('GET', 'trash'); trash = t.items.sort((a: any, b: any) => +new Date(b.time) - +new Date(a.time)); } catch {}
    try { IG = await api('GET', 'ignore'); } catch {}
  }
  $effect(() => { void ui.ov; refresh(); });

  function accTitle(p: string) { return p === '*' ? 'все соседи' : p[0] === '@' ? '👥 ' + ((ui.ov.groups || {})[p.slice(1)]?.title || p) : peerTitle(p); }
  function mountState(m: any): [string, string, string] {
    if (m.revoked) return ['', 'доступ закрыт', ''];
    if (m.hold) return ['bad', 'остановлена', ''];
    if (m.error) return ['warn', m.error.startsWith('нет связи') ? 'нет связи' : 'внимание', m.error];
    if ((m.transfers || []).length) return ['', 'передаю ' + m.transfers.length, ''];
    if ((m.failed || []).length) return ['warn', 'не всё передано', ''];
    return ['ok', 'синхронно', ''];
  }
  const safe = (x: string) => x.replace(/[^\p{L}\p{N}_-]/gu, '_');
  function scopeTitle(s: string) {
    const o = FD && FD.own.find((f: any) => safe(f.id) === s); if (o) return 'ваша: ' + (o.title || o.id);
    const m = FD && FD.mounts.find((x: any) => safe('общие_' + x.peer + '_' + x.id) === s); if (m) return (m.title || m.id) + ' — у ' + peerTitle(m.peer);
    return s;
  }
  async function restore(i: any) { try { const r = await api('POST', 'trash/restore?' + new URLSearchParams({ scope: i.scope, batch: i.batch, path: i.path })); toast('Возвращено: ' + r.path, 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); } }
  async function allowDel(m: any) {
    if (!confirm('Удалить эти файлы и у хозяина папки? Они попадут в его корзину.')) return;
    try { await api('POST', 'mounts/allow-deletes?' + new URLSearchParams({ peer: m.peer, id: m.id })); toast('Разрешено — удаление уйдёт при следующем обмене', 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function saveCfg() {
    const q = new URLSearchParams({ trashMinutes: cfg.trashMin, upMbit: cfg.up, downMbit: cfg.down, maxFileMb: cfg.max });
    try { await api('POST', 'folders/settings?' + q); toast('Сохранено', 'ok'); cfgTouched = false; refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function removeFolder(id: string) {
    if (!confirm('Закрыть доступ к папке «' + id + '»? Файлы у вас останутся, у соседей она перестанет обновляться.')) return;
    try { await api('POST', 'folders/remove?id=' + encodeURIComponent(id)); toast('Доступ закрыт', 'ok'); if (fsCur && fsCur.kind === 'own' && fsCur.id === id) fsCur = null; refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  const editIgnore = (scope: string, peer = '', id = '') => openModal(IgnoreEdit, { scope, peer, id, fd: FD, ig: IG, onsaved: refresh });

  // ── файлы одной папки ──
  let fsCur = $state<{ kind: string; peer: string; id: string } | null>(null);
  let fsFiles = $state<any[]>([]);
  let fsWritable = $state(false);
  let fsHidden = $state(false);
  let fsHiddenN = $state(0);
  let fsQ = $state('');
  let fsSub = $state('');
  let fsUps = $state<{ id: number; path: string; size: number; part: number; err: string }[]>([]);
  let upN = 0;
  let fsCard = $state<HTMLElement>();
  const fsQuery = (extra: Record<string, any> = {}) => {
    const q = new URLSearchParams({ kind: fsCur!.kind, id: fsCur!.id });
    if (fsCur!.peer) q.set('peer', fsCur!.peer);
    for (const k in extra) q.set(k, extra[k]);
    return q;
  };
  function fsShow(kind: string, peer: string, id: string) {
    fsCur = { kind, peer, id };
    fsTab = 'files'; plan = null; fsOpen = new Set();
    fsList();
    setTimeout(() => fsCard?.scrollIntoView({ behavior: 'smooth', block: 'start' }), 30);
  }
  async function fsList() {
    if (!fsCur) return;
    try {
      const r = await api('GET', 'fs/list?' + fsQuery(fsHidden ? { hidden: 1 } : {}));
      fsWritable = r.writable; fsFiles = r.files; fsHiddenN = r.hidden || 0;
    } catch (e: any) { toast(e.message, 'bad'); fsCur = null; }
  }
  const fsTitle = $derived.by(() => {
    if (!fsCur || !FD) return '';
    const t = fsCur.kind === 'own' ? FD.own.find((x: any) => x.id === fsCur!.id) : FD.mounts.find((x: any) => x.peer === fsCur!.peer && x.id === fsCur!.id);
    return ((t && t.title) || fsCur.id) + (fsCur.kind === 'mount' ? ' — у ' + peerTitle(fsCur.peer) : ' — ваша');
  });
  const fsShown = $derived(fsFiles.filter(f => !fsQ || f.path.toLowerCase().includes(fsQ.toLowerCase())).slice(0, 500));
  // (3.3.1) дерево папок — по умолчанию; плоский список — переключателем (и при поиске)
  const fsView = $derived(SET.fsView === 'flat' || fsQ ? 'flat' : 'tree');
  let fsOpen = $state<Set<string>>(new Set());
  type Dir = { name: string; path: string; dirs: Dir[]; files: any[]; count: number; bytes: number; time: string };
  const fsTree = $derived.by(() => {
    const root: Dir = { name: '', path: '', dirs: [], files: [], count: 0, bytes: 0, time: '' };
    const idx = new Map<string, Dir>([['', root]]);
    const dirOf = (p: string): Dir => {
      let d = idx.get(p); if (d) return d;
      const i = p.lastIndexOf('/'), parent = dirOf(i < 0 ? '' : p.slice(0, i));
      d = { name: i < 0 ? p : p.slice(i + 1), path: p, dirs: [], files: [], count: 0, bytes: 0, time: '' };
      parent.dirs.push(d); idx.set(p, d); return d;
    };
    for (const f of fsFiles) {
      const i = f.path.lastIndexOf('/'), d = dirOf(i < 0 ? '' : f.path.slice(0, i));
      d.files.push(f);
      for (let q: string | null = d.path; q !== null; q = q ? (q.includes('/') ? q.slice(0, q.lastIndexOf('/')) : '') : null) {
        const x = idx.get(q)!; x.count++; x.bytes += f.size || 0; if (!x.time || f.time > x.time) x.time = f.time;
        if (!q) break;
      }
    }
    const sort = (d: Dir) => { d.dirs.sort((a, b) => a.name.localeCompare(b.name, 'ru')); d.files.sort((a, b) => a.path.localeCompare(b.path, 'ru')); d.dirs.forEach(sort); };
    sort(root); return root;
  });
  function fsToggle(p: string) { const n = new Set(fsOpen); n.has(p) ? n.delete(p) : n.add(p); fsOpen = n; }
  const fname = (p: string) => p.slice(p.lastIndexOf('/') + 1);
  // (3.3.1) синхронизация папки соседа: что разошлось и что сделает обмен
  let fsTab = $state<'files' | 'sync'>('files');
  let plan = $state<any>(null);
  let planBusy = $state(false);
  async function loadPlan() {
    if (!fsCur || fsCur.kind !== 'mount') return;
    planBusy = true;
    try { plan = await api('GET', 'mounts/plan?' + new URLSearchParams({ peer: fsCur.peer, id: fsCur.id })); } catch (e: any) { toast(e.message, 'bad'); plan = null; }
    planBusy = false;
  }
  async function syncNow() { try { await api('POST', 'sync'); toast('Обмен запущен'); setTimeout(loadPlan, 2500); } catch (e: any) { toast(e.message, 'bad'); } }
  const ACT: Record<string, [string, string]> = { download: ['скачается', 'ok'], update: ['обновится', 'ok'], delete_here: ['удалится здесь (в корзину)', 'warn'], upload: ['уйдёт хозяину', 'acc'], delete_owner: ['удалится у хозяина', 'warn'], restore: ['вернётся копия хозяина — ваша правка уйдёт в «_мои-правки»', 'warn'], conflict: ['правили оба — копия хозяина главная, ваша останется рядом', 'bad'] };
  const eta = (s: number) => (s < 0 ? 'время неизвестно — скорость ещё не измерена' : s < 60 ? 'меньше минуты' : s < 3600 ? '≈ ' + Math.ceil(s / 60) + ' мин' : '≈ ' + (s / 3600).toFixed(1) + ' ч');
  async function fsDel(path: string) {
    if (!confirm('Удалить «' + path + '»? Удаление уйдёт всем, кто подключён к папке. Файл можно вернуть из корзины.')) return;
    try { await api('POST', 'fs/delete?' + fsQuery({ path })); toast('Удалено (в корзине)'); fsList(); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function fsIgn(path: string) {
    const dir = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) + '/' : null;
    const v = prompt('Добавить в игнор этой папки.\n\nФайл: ' + path + (dir ? '\nВся папка: ' + dir : '') + '\nПо имени везде: ' + path.split('/').pop() + '\n\nШаблон:', path);
    if (!v || !fsCur) return;
    try { await igChange('add', { scope: fsCur.kind === 'own' ? 'folder' : 'mount', peer: fsCur.peer, id: fsCur.id, pattern: v.trim() }); toast('Скрыто: ' + v, 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  function fsQueue(list: { file: File; path: string }[]) {
    if (!fsCur || !fsWritable) return;
    const sub = fsSub.trim().replace(/^\/+|\/+$/g, '');
    const target = { ...fsCur };
    let left = list.length;
    for (const it of list) {
      const path = (sub ? sub + '/' : '') + it.path;
      const u = { id: ++upN, path, size: it.file.size, part: 0, err: '' };
      fsUps.unshift(u);
      const row = fsUps[0];
      const q = new URLSearchParams({ kind: target.kind, id: target.id, path });
      if (target.peer) q.set('peer', target.peer);
      const done = () => { if (--left === 0) fsList(); };
      upload('fs/upload?' + q, it.file, p => { row.part = p; })
        .then(() => { row.part = 1; setTimeout(() => { fsUps = fsUps.filter(x => x.id !== u.id); }, 3000); done(); })
        .catch(e => { row.err = e.message; done(); });
    }
  }

  // ── игнор узла ──
  let igAdd = $state('');
  let igTest = $state('');
  let igRes = $state<{ ignored: boolean; rule?: string } | null>(null);
  let igT: any;
  async function addIgnore() {
    const v = igAdd.split(',').map(s => s.trim()).filter(Boolean);
    if (!v.length) return;
    try { for (const p of v) await igChange('add', { scope: 'node', peer: '', id: '', pattern: p }); igAdd = ''; toast('Добавлено в игнор', 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function delIgnore(p: string) { try { await igChange('remove', { scope: 'node', peer: '', id: '', pattern: p }); toast('Убрано из игнора'); refresh(); } catch (e: any) { toast(e.message, 'bad'); } }
  function onIgTest() {
    clearTimeout(igT);
    igT = setTimeout(async () => {
      const v = igTest.trim();
      if (!v) { igRes = null; return; }
      try { igRes = await api('GET', 'ignore/test?' + new URLSearchParams({ path: v, scope: 'node' })); } catch { igRes = null; }
    }, 250);
  }
</script>

{#if FD}
<div class="split">
  <div class="card">
    <div class="row" style="margin-bottom:8px"><h3 style="margin:0">Мои общие папки</h3><div class="grow"></div><button class="b pri" onclick={() => openModal(FolderEdit, { id: null, fd: FD, onsaved: refresh })}>＋ Создать папку</button></div>
    <div class="small muted" style="margin-bottom:8px">Папка лежит у вас. Соседи, которым вы дали доступ, видят её у себя и могут класть, менять и удалять файлы (если доступ «запись»).</div>
    {#each FD.own as f (f.id)}
      <div class="card" style="margin-bottom:8px;padding:10px 12px;box-shadow:none">
        <div class="row"><b><Gi n="folder" /> {f.title || f.id}</b><span class="tag">{f.id}</span><div class="grow"></div>
          <button class="b sm pri" onclick={() => fsShow('own', '', f.id)}>Файлы</button>
          <button class="b sm" data-open="folder" data-id={f.id}><Gi n="folder" /></button>
          <button class="b sm" title="игнор этой папки" onclick={() => editIgnore('folder', '', f.id)}><Gi n="block" />{f.ignore && f.ignore.length ? ' ' + f.ignore.length : ''}</button>
          <button class="b sm" onclick={() => openModal(FolderEdit, { id: f.id, fd: FD, onsaved: refresh })}>Изменить</button>
          <button class="b sm danger" title="закрыть доступ (файлы останутся)" onclick={() => removeFolder(f.id)}>✖</button></div>
        <div class="small muted mono" style="margin:4px 0">{f.root}</div>
        <div class="small">{f.files} файлов · {size(f.bytes)}</div>
        <div style="margin-top:4px">
          {#each Object.entries(f.access || {}) as [p, a]}<span class="chip" class:w={a === 'write'}>{accTitle(p)}: {ACCESS[a as string]}</span>{:else}<span class="small" style="color:var(--warn)">ни у кого нет доступа</span>{/each}
        </div>
      </div>
    {:else}
      <div class="empty">Вы пока ничем не поделились.<br />«＋ Создать папку» — и выберите, кому дать доступ.</div>
    {/each}
  </div>
  <div class="card">
    <div class="row" style="margin-bottom:8px"><h3 style="margin:0">Папки соседей</h3><div class="grow"></div><button class="b sm" data-open="mounts"><Gi n="folder" /> Все</button></div>
    <div class="small muted" style="margin-bottom:8px">Папки, которыми поделились с вами. Копия лежит в «<span class="mono">{FD.mountsRoot}</span>»: работайте с ней как с обычной папкой — изменения уйдут хозяину.</div>
    {#each FD.mounts as m (m.peer + m.id)}
      {@const st = mountState(m)}
      <div class="card" style="margin-bottom:8px;padding:10px 12px;box-shadow:none">
        <div class="row"><b><Gi n="folder" /> {m.title || m.id}</b><span class="small muted">у {peerTitle(m.peer)}</span><span class="chip" class:w={m.access === 'write'}>{ACCESS[m.access]}</span><span class="pill {st[0]}" title={st[2]}>{st[1]}</span><div class="grow"></div>
          <button class="b sm pri" onclick={() => fsShow('mount', m.peer, m.id)}>Файлы</button>
          <button class="b sm" title="игнор этой папки" onclick={() => editIgnore('mount', m.peer, m.id)}><Gi n="block" />{m.ignore.length + m.ownerIgnore.length ? ' ' + (m.ignore.length + m.ownerIgnore.length) : ''}</button>
          <button class="b sm" data-open="mount" data-peer={m.peer} data-id={m.id}><Gi n="folder" /></button></div>
        <div class="small muted" style="margin-top:4px">{m.files} файлов · {size(m.bytes)} · обмен {ago(m.lastSync)}{m.error && !m.hold ? ' · ' + m.error : ''}</div>
        {#if m.hold}
          <div class="card" style="background:var(--bad-soft);border-color:transparent;margin-top:6px;padding:8px 10px;box-shadow:none"><div class="small">{m.holdReason}</div>
            <div class="row" style="margin-top:6px"><button class="b sm danger" onclick={() => allowDel(m)}>Да, удалить</button><span class="small muted">Не хотели удалять? Верните файлы из корзины ниже — папка продолжит работу.</span></div></div>
        {/if}
        {#each m.transfers || [] as t}<div class="small" style="margin-top:4px">{t.up ? '⬆ отправляю' : '⬇ скачиваю'} <b>{t.path}</b> · {size(t.size)} · {t.secs} с — остальные файлы идут, не дожидаясь</div>{/each}
        {#each m.failed || [] as f}<div class="small" style="margin-top:4px;color:var(--warn)">⚠ {f.path}: {f.reason}</div>{/each}
        {#if m.recent && m.recent.length}<details style="margin-top:4px"><summary class="small">Последние изменения</summary>{#each m.recent as r}<div class="small mono">{r}</div>{/each}</details>{/if}
      </div>
    {:else}
      <div class="empty">С вами пока не поделились папками.{#if !FD.mountsEnabled}<br />Подключение папок соседей выключено (mounts.enabled).{/if}</div>
    {/each}
  </div>
</div>

{#if fsCur}
  <div class="card" style="margin-top:14px" bind:this={fsCard}>
    <div class="row"><h3 style="margin:0"><Gi n="folder" /> {fsTitle}</h3>{#if fsWritable}<span class="chip w">можно менять</span>{:else}<span class="chip">только чтение</span>{/if}<div class="grow"></div>
      <label class="small muted" class:hide={!fsHiddenN && !fsHidden}><input type="checkbox" bind:checked={fsHidden} onchange={fsList} /> скрытые {fsHiddenN ? '(' + fsHiddenN + ')' : ''}</label>
      <button class="b sm" title="игнор этой папки" onclick={() => editIgnore(fsCur!.kind === 'own' ? 'folder' : 'mount', fsCur!.peer, fsCur!.id)}><Gi n="block" /> Игнор</button>
      <span class="seg"><button class:on={fsView === 'tree'} title="Папки деревом" onclick={() => setSet('fsView', 'tree')}>дерево</button><button class:on={fsView === 'flat'} title="Все файлы списком" onclick={() => setSet('fsView', 'flat')}>списком</button></span>
      <input bind:value={fsQ} placeholder="поиск" style="width:160px" />
      <button class="b sm" data-open={fsCur.kind === 'own' ? 'folder' : 'mount'} data-id={fsCur.id} data-peer={fsCur.peer || undefined}><Gi n="folder" /> Открыть в Проводнике</button>
      <button class="b sm" onclick={() => (fsCur = null)}>✖</button></div>
    {#if fsCur.kind === 'mount'}
      <div class="tabs fstabs" role="tablist"><button role="tab" class:on={fsTab === 'files'} onclick={() => (fsTab = 'files')}>Файлы</button><button role="tab" class:on={fsTab === 'sync'} onclick={() => { fsTab = 'sync'; loadPlan(); }}>Синхронизация{plan && plan.total ? ' · ' + plan.total : ''}</button></div>
    {/if}
    {#if fsTab === 'sync' && fsCur.kind === 'mount'}
      <div class="fsplan">
        {#if !plan}<div class="empty">{planBusy ? 'Сверяю с хозяином…' : 'Нет данных'}</div>
        {:else}
          <div class="row" style="gap:10px;flex-wrap:wrap">
            {#if plan.total}
              {#if plan.down}<span class="pill ok">⬇ скачается {size(plan.down)} · {eta(plan.etaDown)}</span>{/if}
              {#if plan.up}<span class="pill acc">⬆ уйдёт хозяину {size(plan.up)} · {eta(plan.etaUp)}</span>{/if}
              <span class="small muted">{plan.total} {plan.total === 1 ? 'файл' : 'файлов'} разошлось · здесь {plan.localFiles}, у хозяина {plan.remoteFiles}</span>
            {:else}<span class="pill ok">✓ всё совпадает с хозяином</span><span class="small muted">здесь {plan.localFiles} файлов, у хозяина {plan.remoteFiles}</span>{/if}
            <div class="grow"></div><span class="small muted">обмен {ago(plan.lastSync)}</span>
            <button class="b sm" onclick={loadPlan} disabled={planBusy}>Проверить</button><button class="b sm pri" onclick={syncNow}>Обменяться сейчас</button>
          </div>
          {#if plan.hold}<div class="small" style="color:var(--bad);margin-top:6px">Остановлена: {plan.holdReason}</div>{/if}
          {#each plan.transfers || [] as t}<div class="small" style="margin-top:4px">{t.up ? '⬆ отправляю' : '⬇ скачиваю'} <b>{t.path}</b> · {size(t.size)} · {t.secs} с</div>{/each}
          {#each plan.failed || [] as f}<div class="small" style="margin-top:4px;color:var(--warn)">⚠ {f.path}: {f.reason}</div>{/each}
          {#if plan.items.length}
            <table style="margin-top:8px"><thead><tr><th>Файл</th><th>Что будет</th><th>Здесь</th><th>У хозяина</th></tr></thead><tbody>
              {#each plan.items as it (it.path)}
                <tr class="fs-row"><td class="mono small">{it.path}</td><td><span class="pill {ACT[it.act]?.[1] || ''}">{ACT[it.act]?.[0] || it.act}</span></td>
                  <td class="small muted">{#if it.local}{size(it.local.size)} · <span title={fullTime(it.local.time)}>{ago(it.local.time)}</span>{:else}—{/if}</td>
                  <td class="small muted">{#if it.remote}{size(it.remote.size)} · <span title={fullTime(it.remote.time)}>{ago(it.remote.time)}</span>{:else}—{/if}</td></tr>
              {/each}
            </tbody></table>
            {#if plan.total > plan.items.length}<div class="small muted">показаны первые {plan.items.length} из {plan.total}</div>{/if}
          {/if}
        {/if}
      </div>
    {:else}
    {#if fsWritable}
      <Drop onfiles={fsQueue} style="margin-top:10px;padding:12px">
        <b>Перетащите файлы или папки сюда</b> — они сразу появятся в этой папке у всех, кто к ней подключён
        <div class="row small" style="justify-content:center;margin-top:6px"><span>во вложенную папку</span><input bind:value={fsSub} placeholder="необязательно" style="width:180px" onclick={e => e.stopPropagation()} /></div>
      </Drop>
    {/if}
    <div>
      {#each fsUps as u (u.id)}
        <div style="margin:6px 0"><div class="row small"><span class="mono grow">{u.path}</span><span class="muted">{size(u.size)}</span></div>
          {#if u.err}<div class="small" style="color:var(--bad)">{u.err}</div>{:else}<div class="bar"><i style="width:{u.part * 100}%"></i></div>{/if}</div>
      {/each}
    </div>
    <table style="margin-top:8px">
      <thead><tr><th>Файл</th><th>Размер</th><th>Изменён</th><th></th></tr></thead>
      <tbody>
        {#if fsView === 'tree'}
          {@render dirRows(fsTree, 0)}
          {#if !fsFiles.length}<tr><td colspan="4" class="empty">Папка пуста{fsWritable ? ' — перетащите сюда файлы' : ''}</td></tr>{/if}
        {:else}
        {#each fsShown as f (f.path)}
          <tr class="fs-row" class:ign={f.ignored} title={f.ignored ? 'в игноре: ' + f.ignored : undefined}>
            <td class="mono"><a href="#dl" onclick={e => { e.preventDefault(); download('fs/file?' + fsQuery({ path: f.path }), f.path.split('/').pop()).catch(er => toast(er.message, 'bad')); }}>{f.path}</a></td>
            <td class="small muted">{size(f.size)}</td><td class="small muted" title={fullTime(f.time)}>{ago(f.time)}</td>
            <td>{#if f.ignored}<span class="chip ig">{f.ignored}</span>{:else}<button class="b sm" title="в игнор" onclick={() => fsIgn(f.path)}><Gi n="block" /></button> {/if}
              {#if fsWritable && !f.path.startsWith('_мои-правки/')}<button class="b sm danger" title="удалить (в корзину)" onclick={() => fsDel(f.path)}><Gi n="trash" /></button>{/if}</td>
          </tr>
        {:else}
          <tr><td colspan="4" class="empty">{fsQ ? 'Ничего не нашлось' : 'Папка пуста' + (fsWritable ? ' — перетащите сюда файлы' : '')}</td></tr>
        {/each}
        {/if}
      </tbody>
    </table>
    {/if}
  </div>
{/if}

{#snippet fileRow(f: any, depth: number)}
  <tr class="fs-row" class:ign={f.ignored} title={f.ignored ? 'в игноре: ' + f.ignored : undefined}>
    <td class="mono" style="padding-left:{8 + depth * 18}px"><a href="#dl" onclick={e => { e.preventDefault(); download('fs/file?' + fsQuery({ path: f.path }), fname(f.path)).catch(er => toast(er.message, 'bad')); }}>{depth || fsView === 'tree' ? fname(f.path) : f.path}</a></td>
    <td class="small muted">{size(f.size)}</td><td class="small muted" title={fullTime(f.time)}>{ago(f.time)}</td>
    <td>{#if f.ignored}<span class="chip ig">{f.ignored}</span>{:else}<button class="b sm" title="в игнор" onclick={() => fsIgn(f.path)}><Gi n="block" /></button> {/if}
      {#if fsWritable && !f.path.startsWith('_мои-правки/')}<button class="b sm danger" title="удалить (в корзину)" onclick={() => fsDel(f.path)}><Gi n="trash" /></button>{/if}</td>
  </tr>
{/snippet}
{#snippet dirRows(d: any, depth: number)}
  {#each d.dirs as sub (sub.path)}
    {@const open = fsOpen.has(sub.path)}
    <tr class="fs-row fs-dir" onclick={() => fsToggle(sub.path)}>
      <td style="padding-left:{8 + depth * 18}px"><span class="fs-caret">{open ? '▾' : '▸'}</span> <Gi n="folder" /> <b>{sub.name}</b></td>
      <td class="small muted">{size(sub.bytes)}</td><td class="small muted" title={fullTime(sub.time)}>{ago(sub.time)}</td>
      <td class="small muted">{sub.count} {sub.count === 1 ? 'файл' : 'файлов'}</td>
    </tr>
    {#if open}{@render dirRows(sub, depth + 1)}{/if}
  {/each}
  {#each d.files.slice(0, 1000) as f (f.path)}{@render fileRow(f, depth)}{/each}
{/snippet}

<div class="card" style="margin-top:14px">
  <div class="row"><h3 style="margin:0"><Gi n="block" /> Игнор для всего узла</h3><div class="grow"></div><button class="b sm" onclick={() => editIgnore('node')}>Изменить списком</button></div>
  <div class="small muted" style="margin:6px 0">Файлы и папки по этим шаблонам мост не видит: не показывает, не передаёт и не принимает — в общих папках, ручной отправке и пространствах. Для одной папки — кнопка «<Gi n="block" />» у папки.</div>
  <div>
    {#each IG ? IG.node : [] as p}<span class="chip ig">{p}<span class="x" title="убрать из игнора" onclick={() => delIgnore(p)} role="button" tabindex="0" onkeydown={() => {}}>✕</span></span>
    {:else}<span class="small muted">Пусто — мост видит всё (кроме временных файлов: ~$…, *.tmp, Thumbs.db).</span>{/each}
  </div>
  <div class="row" style="margin-top:8px"><input bind:value={igAdd} onkeydown={e => e.key === 'Enter' && addIgnore()} placeholder="например: *.bak, Сборки/, секрет.txt" style="flex:1;min-width:200px" /><button class="b pri" onclick={addIgnore}>＋ В игнор</button>
    <input bind:value={igTest} oninput={onIgTest} placeholder="проверить путь: папка/файл.txt" style="flex:1;min-width:200px" />
    <span class="small">{#if igRes}{#if igRes.ignored}<span style="color:var(--warn)">скрыт: {igRes.rule}</span>{:else}<span style="color:var(--ok)">виден</span>{/if}{/if}</span></div>
  <details style="margin-top:6px"><summary class="small">Как писать шаблоны</summary><ul class="small" style="margin:6px 0">{#each IG ? IG.syntax : [] as s}<li>{s}</li>{/each}</ul></details>
</div>

<div class="split" style="margin-top:14px">
  <div class="card"><div class="row"><h3 style="margin:0"><Gi n="trash" /> Корзина</h3><div class="grow"></div><button class="b sm" data-open="trash"><Gi n="folder" /> Папка</button></div>
    <div class="small muted" style="margin:6px 0">Всё, что удалено или заменено в общих папках, сначала попадает сюда. Отсюда можно вернуть.</div>
    {#if trash.length}
      <table><tbody>{#each trash.slice(0, 100) as i (i.scope + i.batch + i.path)}
        <tr class="fs-row"><td class="mono small">{i.path}<div class="muted">{scopeTitle(i.scope)}</div></td><td class="small muted">{size(i.size)}</td>
          <td class="small muted" title={fullTime(i.time)}>{ago(i.time)}</td><td><button class="b sm" onclick={() => restore(i)}>↩ Вернуть</button></td></tr>
      {/each}</tbody></table>
    {:else}<div class="empty">Корзина пуста</div>{/if}
  </div>
  <div class="card"><h3>Настройки передачи</h3>
    <div class="row" style="gap:14px;align-items:flex-end">
      <label class="f" style="margin:0">Скорость отдачи, Мбит/с<input bind:value={cfg.up} oninput={() => (cfgTouched = true)} inputmode="decimal" placeholder="без ограничения" style="width:150px" /></label>
      <label class="f" style="margin:0">Скорость приёма, Мбит/с<input bind:value={cfg.down} oninput={() => (cfgTouched = true)} inputmode="decimal" placeholder="без ограничения" style="width:150px" /></label>
    </div>
    <div class="small muted" style="margin:4px 0 10px">Общая на все передачи (папки, отправка, пространства). Пусто — без ограничения. Пример: 50 — около 6 МБ/с, чтобы мост не забивал канал.</div>
    <div class="row" style="gap:14px;align-items:flex-end">
      <label class="f" style="margin:0">Предел размера файла, МБ<input bind:value={cfg.max} oninput={() => (cfgTouched = true)} inputmode="numeric" placeholder="без предела" style="width:150px" /></label>
      <label class="f" style="margin:0">Корзина, минут<input bind:value={cfg.trashMin} oninput={() => (cfgTouched = true)} type="number" min="0" max="525600" style="width:120px" /></label>
      <button class="b pri" onclick={saveCfg}>Сохранить</button>
    </div>
    <div class="small muted" style="margin-top:4px">Предел: пусто — любой размер. Корзина: по умолчанию 10 минут, 0 — удалять сразу (не советуем), 1440 — сутки.</div>
  </div>
</div>
{:else}
  <div class="empty">Загружаю…</div>
{/if}
