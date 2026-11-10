<script lang="ts">
  import { ui, toast, titleOf, badgeCount } from '../lib/state.svelte';
  import { api, aurl } from '../lib/api';
  import { openLightbox } from '../lib/lightbox.svelte';
  import { ago } from '../lib/util';
  import Gi from '../components/Gi.svelte';
  import AgentAsk from '../components/AgentAsk.svelte';

  const AGST: Record<string, [string, string]> = { queued: ['ждёт, пока возьмут', 'warn'], running: ['в работе', 'acc'], waiting: ['ожидание', 'warn'], idle: ['готово', 'ok'], failed: ['не получилось', 'bad'], stopped: ['остановлено', ''] };
  const live = (st: string) => st === 'running' || st === 'queued' || st === 'waiting';
  let AG = $state<any>(null);
  let err = $state('');
  let peer = $state('');
  let prompt = $state('');
  let cur = $state<string | null>(null);
  let th = $state<any>(null);
  let events = $state<any[]>([]);
  let after = -1;
  let more = $state('');
  let dirty = false;
  let cfg = $state<any>({ pick: 'off', mode: 'off', allowFrom: [], workDir: '', claude: '', permissionMode: 'auto', maxMinutes: 60, maxParallel: 3, maxBudgetUsd: '', engine: 'claude', codex: '', codexSandbox: 'workspace-write', codexNetwork: false, codexModel: '', codexDirs: '' });
  let check = $state<any>(null);
  let log = $state<HTMLElement>();
  let timer: any = null;

  // переход из «Записей»: открыть нить задачи
  $effect(() => { if (ui.agentThread) { const id = ui.agentThread; ui.agentThread = ''; show(id); } });
  async function refresh() {
    try { AG = await api('GET', 'agent'); err = ''; } catch (e: any) { err = e.message; return; }
    const can = cands(AG);
    if (!peer || !can.some((p: any) => p.name === peer)) peer = can[0]?.name || '';
    badgeCount('agent', AG.threads.filter((t: any) => (t.role === 'in' && t.status === 'queued') || (live(t.status) && t.open)).length);
    if (!dirty) {
      const st = AG.settings;
      cfg = { mode: st.mode, allowFrom: [...st.allowFrom], workDir: st.workDir, claude: st.claude, permissionMode: st.permissionMode || 'auto', maxMinutes: st.maxMinutes, maxParallel: st.maxParallel, maxBudgetUsd: st.maxBudgetUsd > 0 ? st.maxBudgetUsd : '',
        pick: st.mode === 'auto' ? (st.engine === 'codex' ? 'codex' : 'auto') : st.mode,
        engine: st.engine === 'codex' ? 'codex' : 'claude', codex: st.codex || '', codexSandbox: st.codexSandbox || 'workspace-write', codexNetwork: !!st.codexNetwork, codexModel: st.codexModel || '', codexDirs: (st.codexDirs || []).join('\n') };
    }
    if (cur) show(cur, true);
    clearTimeout(timer);
    if (ui.tab === 'agent') timer = setTimeout(refresh, AG.threads.some((t: any) => live(t.status)) ? 1500 : 5000);
  }
  $effect(() => { refresh(); return () => clearTimeout(timer); });
  // кому поручать: этот ПК (если агент здесь включён) и соседи, которые принимают
  function cands(a: any): any[] {
    const l = a.local && a.local.mode !== 'off' ? [{ name: a.local.name, title: 'Этот ПК', online: true, agent: a.local.mode, engine: a.local.engine, local: true }] : [];
    return l.concat(a.peers.filter((p: any) => p.agent));
  }
  const can = $derived(AG ? cands(AG) : []);
  const isLocal = (n: string) => !!AG?.local && AG.local.name === n;
  const who = (n: string) => isLocal(n) ? 'этому ПК' : titleOf(n);
  const eng = (e: string) => e === 'codex' ? 'Codex' : 'Claude';
  const agentLine = (p: any) => p.agent === 'auto' ? eng(p.engine) + ' запустится сам' : 'ждёт сессию Claude';
  // Codex сообщает токены, а не деньги
  const kTok = (n: number) => n >= 1e6 ? (n / 1e6).toFixed(1) + 'M' : n >= 1e3 ? (n / 1e3).toFixed(1) + 'k' : String(n);
  const spent = (t: any) => t.cost ? ' · $' + t.cost.toFixed(2) : t.tokens && (t.tokens.input + t.tokens.output) ? ' · ' + kTok(t.tokens.input + t.tokens.output) + ' ток.' : '';
  const spentTip = (t: any) => t.tokens && (t.tokens.input + t.tokens.output) ? `вход ${t.tokens.input} (из кэша ${t.tokens.cached}), выход ${t.tokens.output}` : '';
  const imgIndex = (path: string) => (th?.images || []).findIndex((i: any) => i.path === path);
  const imgSrc = (n: number) => aurl('agent/image?id=' + encodeURIComponent(cur!) + '&n=' + n + '&v=' + (th?.images?.[n]?.size || 0));
  function zoom(n: number) { const im = th.images[n]; const src = imgSrc(n); openLightbox({ src, dl: src, name: im.path.split('/').pop(), sticker: '' }); }
  async function codexCheck() { check = { busy: true }; try { check = await api('GET', 'agent/codex-check'); } catch (e: any) { check = { error: e.message }; } }

  async function show(id: string, keep = false) {
    if (!keep || id !== cur) { cur = id; after = -1; events = []; th = null; }
    let t: any;
    try { t = await api('GET', 'agent/thread?id=' + encodeURIComponent(id) + '&after=' + after); } catch (e: any) { toast(e.message, 'bad'); return; }
    if (id !== cur) return;
    const atBottom = !log || log.scrollHeight - log.scrollTop - log.clientHeight < 40;
    events = events.concat(t.events); after = t.lastN; th = t;
    if (atBottom) requestAnimationFrame(() => { if (log) log.scrollTop = log.scrollHeight; });
  }
  const busy = $derived(th && live(th.status));
  async function ask() {
    if (!prompt.trim()) { toast('Напишите, что сделать', 'bad'); return; }
    try { const t = await api('POST', 'agent/ask', { peer: isLocal(peer) ? '' : peer, prompt: prompt.trim() }); prompt = ''; cur = t.id; after = -1; events = []; toast('Поручено ' + who(peer), 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function stop() { try { await api('POST', 'agent/stop?id=' + encodeURIComponent(cur!)); toast('Остановка отправлена'); } catch (e: any) { toast(e.message, 'bad'); } }
  async function del() { if (!confirm('Убрать нить из списка на этом ПК?')) return; await api('POST', 'agent/delete?id=' + encodeURIComponent(cur!)); cur = null; th = null; refresh(); }
  async function cont() { const p = more.trim(); if (!p) return; try { await api('POST', 'agent/ask', { peer: th.local ? '' : th.peer, prompt: p, thread: cur }); more = ''; refresh(); } catch (e: any) { toast(e.message, 'bad'); } }
  async function save() {
    // режим и движок выбираются одним переключателем: выключен / сам запускает Claude / сам запускает Codex / ждёт сессию Claude
    cfg.mode = cfg.pick === 'codex' ? 'auto' : cfg.pick;
    const engine = cfg.pick === 'codex' ? 'codex' : 'claude';
    cfg.engine = engine;
    const body = { mode: cfg.mode, allowFrom: cfg.allowFrom, workDir: String(cfg.workDir).trim(), claude: String(cfg.claude).trim(), permissionMode: cfg.permissionMode, maxMinutes: +cfg.maxMinutes || 60, maxParallel: +cfg.maxParallel || 3, maxBudgetUsd: +String(cfg.maxBudgetUsd).replace(',', '.') || 0,
      engine, codex: String(cfg.codex).trim(), codexSandbox: cfg.codexSandbox, codexNetwork: !!cfg.codexNetwork, codexModel: String(cfg.codexModel).trim(), codexDirs: String(cfg.codexDirs).split('\n').map((x: string) => x.trim()).filter(Boolean) };
    try { await api('POST', 'agent/settings', body); dirty = false; toast('Сохранено', 'ok'); (document.activeElement as HTMLElement)?.blur(); refresh(); if (engine === 'codex') codexCheck(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function exchange(p: string) { try { await api('POST', 'agent/exchange?peer=' + encodeURIComponent(p)); toast('Обменная папка открыта для ' + titleOf(p), 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); } }
  // канал для агента без сети (VM): файловый канал efc/1 с теми же инструментами, что bridge mcp
  async function fileChannel(on: boolean) {
    try { await api('POST', 'agent/settings', { fileChannel: on }); toast(on ? 'Канал для агента включён' : 'Канал для агента выключен', 'ok'); setTimeout(refresh, 600); } catch (e: any) { toast(e.message, 'bad'); refresh(); }
  }
  function fcState(fc: any): [string, string] {
    if (!fc || !fc.on) return ['выключен', 'muted'];
    if (fc.running) return ['работает: ' + fc.path, 'ok'];
    if (fc.error) return ['не открыт: ' + fc.error, 'bad'];
    return ['открывается…', 'muted'];
  }
  async function copyText(t: string) { try { await navigator.clipboard.writeText(t); toast('Скопировано', 'ok'); } catch { toast('Не удалось скопировать — выделите вручную', 'bad'); } }
  const pill = (st: string) => AGST[st] || [st, ''];
  const lines = (x: string) => String(x || '').split('\n');
</script>

<div class="rooms">
  <div>
    <div class="card"><h3>Поручить</h3>
      <label class="f">Кому<select bind:value={peer}>{#each can as p (p.name)}<option value={p.name}>{(p.local ? 'Этот ПК' : p.title || p.name) + (p.online ? '' : ' (нет связи)') + ' — ' + agentLine(p)}</option>{/each}</select></label>
      <label class="f">Что сделать<textarea bind:value={prompt} rows="5" placeholder="Например: прогони тесты на стенде, положи лог и скриншоты падений в обменную папку, итогом — список упавших."></textarea></label>
      <div class="row"><button class="b pri" disabled={!can.length} onclick={ask}>Поручить</button><span class="small muted">{AG && !can.length ? 'Ни один сосед не принимает поручения от этого ПК: на его панели «Агенты» → режим и «кто может поручать».' : ''}</span></div>
    </div>
    <div class="card" id="agThreads" style="margin-top:14px"><h3>Нити</h3>
      {#if err}<div class="pill bad">{err}</div>
      {:else if AG}
        {#each AG.threads as t (t.id)}
          <div class="chan" class:on={t.id === cur} style="cursor:pointer" role="button" tabindex="0" onclick={() => show(t.id)} onkeydown={() => {}}>
            <div class="row"><b style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:62%">{t.title || t.id}</b><div class="grow"></div><span class="pill {pill(t.status)[1]}">{pill(t.status)[0]}</span></div>
            <div class="small muted" title={spentTip(t)}>{t.local ? '⟲ на этом ПК' : t.role === 'out' ? '→ ' + titleOf(t.peer) : '← от ' + titleOf(t.peer)}{t.engine === 'codex' ? ' · Codex' : ''} · {ago(t.updated)}{spent(t)}{t.images && t.images.length ? ' · 🖼 ' + t.images.length : ''}</div>
            {#if live(t.status) && t.open}<div class="small ag-phase wait">❓ ждёт ответа — откройте нить</div>
            {:else if live(t.status) && t.phase}<div class="small ag-phase" class:wait={t.status === 'waiting'}>{t.status === 'waiting' ? '⏳ ' : '▸ '}{t.phase}</div>
            {:else if t.last}<div class="small" style="margin-top:4px;opacity:.85;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{t.last}</div>{/if}
            {#if t.record && t.record.id}<div class="small muted">задача {t.record.id}</div>{/if}
          </div>
        {:else}<div class="empty">Поручений пока не было.</div>{/each}
      {/if}
    </div>
  </div>
  <div>
    <div class="card">
      {#if !th}<div class="empty">Выберите нить слева — здесь будет ход работы в реальном времени.</div>
      {:else}
        <div class="row" style="margin-bottom:10px"><b style="font-size:15px">{th.title || th.id}</b><div class="grow"></div><span class="pill {pill(th.status)[1]}">{pill(th.status)[0]}</span>
          {#if busy}<button class="b sm danger" onclick={stop}><Gi n="stop" /> Остановить</button>{/if}
          <button class="b sm" title="убрать из списка на этом ПК" onclick={del}><Gi n="trash" /></button></div>
        <div class="small muted" style="margin-bottom:8px" title={spentTip(th)}>{th.local ? 'поручено этому ПК' : th.role === 'out' ? 'поручено ' + titleOf(th.peer) : 'поручение от ' + titleOf(th.peer)} · {eng(th.engine)}{spent(th)}{#if th.exchangePath} · обмен: <span class="mono">{th.exchangePath}</span>{/if}</div>
        {#if th.role === 'in' && th.status === 'queued' && !th.taken}
          <div class="banner" style="margin-bottom:10px"><span>✦</span><div class="grow small">Ждёт сессию Claude на этом ПК: откройте Claude (Desktop → Code) с подключёнными инструментами моста и скажите «возьми задачу из моста».</div></div>
        {/if}
        {#if busy}<AgentAsk {th} onanswered={() => show(cur!, true)} />{/if}
        <div class="log ag" bind:this={log}>
          {#each events as e, i (i)}
            {#if e.kind === 'prompt'}<div class="ag-p"><span class="small muted">поручение · {ago(e.t)}</span><div>{#each lines(e.text) as l, j}{#if j}<br />{/if}{l}{/each}</div></div>
            {:else if e.kind === 'tool' && e.text.startsWith('Картинка: ')}
              {#each [imgIndex(e.text.slice(10))] as n}
                <div class="ag-t">› {e.text}{#if n >= 0 && th.images[n].w} · {th.images[n].w}×{th.images[n].h}{/if}</div>
                {#if n >= 0}<button class="ag-img" title="во весь экран" onclick={() => zoom(n)}><img src={imgSrc(n)} alt="" loading="lazy" onerror={e => ((e.currentTarget as HTMLElement).parentElement!.classList.add('nofile'))} /><span>ждёт синхронизации</span></button>{/if}
              {/each}
            {:else if e.kind === 'tool'}<div class="ag-t">› {e.text}</div>
            {:else if e.kind === 'result'}<div class="ag-r"><span class="small muted">итог</span><div>{#each lines(e.text) as l, j}{#if j}<br />{/if}{l}{/each}</div></div>
            {:else if e.kind === 'error'}<div class="ag-e">{#each lines(e.text) as l, j}{#if j}<br />{/if}{l}{/each}</div>
            {:else if e.kind === 'info'}<div class="ag-i">{e.text}</div>
            {:else if e.kind === 'phase'}<div class="ag-ph">◆ {e.text}</div>
            {:else if e.kind === 'ask'}<div class="ag-ask">❓ {#each lines(e.text) as l, j}{#if j}<br />{/if}{l}{/each}</div>
            {:else if e.kind === 'answer'}<div class="ag-ans">↳ {e.text}</div>
            {:else}<div class="ag-x">{#each lines(e.text) as l, j}{#if j}<br />{/if}{l}{/each}</div>{/if}
          {/each}
          {#if busy}<div class="ag-i ag-live">● {th.status === 'queued' ? 'ждёт, пока возьмут' : th.status === 'waiting' ? 'ожидание' + (th.phase ? ': ' + th.phase : '') : 'работает' + (th.phase ? ': ' + th.phase : '')}…</div>{/if}
        </div>
        {#if th.images && th.images.length > 1}
          <div class="small muted" style="margin-top:10px">Картинки нити · {th.images.length}</div>
          <div class="ag-strip">{#each th.images as im, n (im.path)}<button class="ag-img" title={im.path + (im.w ? ' · ' + im.w + '×' + im.h : '')} onclick={() => zoom(n)}><img src={imgSrc(n)} alt="" loading="lazy" onerror={e => ((e.currentTarget as HTMLElement).parentElement!.classList.add('nofile'))} /><span>ждёт синхронизации</span></button>{/each}</div>
        {/if}
        {#if th.role === 'out' && !busy}
          <div class="row" style="margin-top:10px"><textarea bind:value={more} rows="2" placeholder="Продолжить в той же сессии: что дальше?"></textarea></div>
          <div class="row" style="margin-top:6px"><div class="grow"></div><button class="b pri sm" onclick={cont}>Продолжить</button></div>
        {/if}
      {/if}
    </div>
    {#if AG}
      <div class="grid" style="margin-top:14px" onchange={() => (dirty = true)} oninput={() => (dirty = true)} role="group">
        <div class="card"><h3>Этот ПК — исполнитель</h3>
          <div class="row" style="margin-bottom:10px">
            <label class="chip"><input type="radio" value="off" bind:group={cfg.pick} /> выключен</label>
            <label class="chip"><input type="radio" value="auto" bind:group={cfg.pick} /> сам запускает Claude</label>
            <label class="chip"><input type="radio" value="codex" bind:group={cfg.pick} /> сам запускает Codex</label>
            <label class="chip"><input type="radio" value="manual" bind:group={cfg.pick} /> ждёт сессию Claude</label>
          </div>
          <div class="f">Кто может поручать<div class="row">
            <label class="chip"><input type="checkbox" value="*" bind:group={cfg.allowFrom} /> все соседи</label>
            {#each Object.entries(ui.ov.groups || {}) as [g, v]}<label class="chip"><input type="checkbox" value={'@' + g} bind:group={cfg.allowFrom} /> 👥 {(v as any).title || g}</label>{/each}
            {#each AG.peers as p (p.name)}<label class="chip"><input type="checkbox" value={p.name} bind:group={cfg.allowFrom} /> {p.title || p.name}</label>{/each}
          </div></div>
          <label class="f">Рабочая папка {cfg.pick === 'codex' ? 'Codex' : 'Claude'}<input bind:value={cfg.workDir} placeholder="пусто — папка «agent» рядом с мостом" /></label>
          <div class:hide={cfg.pick !== 'codex'}>
            <label class="f">Codex (codex.exe)<input bind:value={cfg.codex} placeholder="пусто — найти самому" /></label>
            <div class="small" style="margin:-4px 0 10px">{#if AG.codex}<span style="color:var(--ok)">найден: {AG.codex}</span>{:else}<span style="color:var(--warn)">Codex не найден. Установите: PowerShell → <code>irm https://chatgpt.com/codex/install.ps1 | iex</code> (или <code>npm i -g @openai/codex</code>), затем один раз <code>codex login</code>.</span>{/if}</div>
            <div class="row" style="gap:12px;align-items:flex-end">
              <label class="f" style="margin:0">Песочница<select bind:value={cfg.codexSandbox}><option value="workspace-write">workspace-write — пишет только в рабочую и обменную папки</option><option value="read-only">read-only — только читает</option><option value="danger-full-access">danger-full-access — без ограничений</option></select></label>
              <label class="f" style="margin:0">Модель<input bind:value={cfg.codexModel} placeholder="по умолчанию" style="width:150px" /></label>
              <label class="f" style="margin:0">Минут на поручение<input bind:value={cfg.maxMinutes} type="number" min="1" style="width:90px" /></label>
              <label class="f" style="margin:0">Одновременно<input bind:value={cfg.maxParallel} type="number" min="1" max="10" style="width:80px" /></label>
            </div>
            <label class="chip" style="margin-top:10px"><input type="checkbox" bind:checked={cfg.codexNetwork} /> сеть для команд Codex (картинкам не нужна)</label>
            <label class="f" style="margin-top:10px">Ещё папки, куда Codex может писать — по одной на строку<textarea bind:value={cfg.codexDirs} rows="2" placeholder="например C:\Projects\Orven\Content"></textarea></label>
            <div class="row" style="gap:10px;align-items:center"><button class="b sm" onclick={codexCheck}>Проверить Codex</button>
              {#if check?.busy}<span class="small muted">проверяю…</span>
              {:else if check && check.found && !check.error}<span class="small" style="color:var(--ok)">{check.version} · {check.login}</span>
              {:else if check}<span class="small" style="color:var(--bad)">{check.error}</span>{/if}</div>
          </div>
          <div class:hide={cfg.pick !== 'auto'}>
            <label class="f">Claude Code (claude.exe)<input bind:value={cfg.claude} placeholder="пусто — найти самому" /></label>
            <div class="small" style="margin:-4px 0 10px">{#if AG.claude}<span style="color:var(--ok)">найден: {AG.claude}</span>{:else}<span style="color:var(--warn)">Claude Code не найден. Установите: PowerShell → <code>irm https://claude.ai/install.ps1 | iex</code>, затем один раз запустите <code>claude</code> и войдите тем же аккаунтом.</span>{/if}</div>
            <div class="row" style="gap:12px;align-items:flex-end">
              <label class="f" style="margin:0">Разрешения<select bind:value={cfg.permissionMode}><option value="auto">auto — каждое действие проверяет классификатор</option><option value="acceptEdits">acceptEdits — правки файлов без вопросов</option><option value="bypassPermissions">bypassPermissions — всё без проверок</option></select></label>
              <label class="f" style="margin:0">Минут на поручение<input bind:value={cfg.maxMinutes} type="number" min="1" style="width:90px" /></label>
              <label class="f" style="margin:0">Одновременно<input bind:value={cfg.maxParallel} type="number" min="1" max="10" style="width:80px" /></label>
              <label class="f" style="margin:0">Предел, $<input bind:value={cfg.maxBudgetUsd} inputmode="decimal" placeholder="нет" style="width:80px" /></label>
            </div>
          </div>
          <div class="row" style="margin-top:12px"><div class="grow"></div><button class="b pri" onclick={save}>Сохранить</button></div>
        </div>
        <div class="card"><h3>Подключить к Claude</h3>
          <div class="small muted" style="margin-bottom:8px">Чтобы Claude на этом ПК мог поручать задачи соседям (или брать поручения в ручном режиме), дайте ему инструменты моста. Одна команда в консоли — для всех проектов:</div>
          <pre class="log" style="max-height:none">{AG.mcp.cli}</pre>
          <div class="row" style="margin:6px 0 12px"><button class="b sm" onclick={() => copyText(AG.mcp.cli)}>Скопировать</button><span class="small muted">или файл <code>.mcp.json</code> в папке проекта (Claude Desktop → Code тоже его читает):</span></div>
          <pre class="log" style="max-height:none">{JSON.stringify(AG.mcp.json, null, 2)}</pre>
          <div class="row" style="margin-top:6px"><button class="b sm" onclick={() => copyText(JSON.stringify(AG.mcp.json, null, 2))}>Скопировать</button></div>
          <div class="small" style="margin-top:14px"><b>Обменная папка</b> — общая папка, куда оба Claude кладут файлы: логи, отчёты, скриншоты.</div>
          <div class="row" style="margin-top:6px">
            {#if AG.exchange}<span class="small">«{AG.exchange.id}»: <span class="mono">{AG.exchange.path}</span> — доступ: {Object.keys(AG.exchange.access).map(titleOf).join(', ') || 'ни у кого'}</span>{:else}<span class="small muted">не настроена</span>{/if}
            {#each AG.peers.filter((p: any) => !AG.exchange || !AG.exchange.access[p.name]) as p (p.name)}<button class="b sm" onclick={() => exchange(p.name)}>＋ обмен с {p.title || p.name}</button>{/each}
          </div>
          <div class="small" style="margin-top:14px"><b>Агент без сети</b> — Claude в виртуальной машине (Cowork и т. п.) не достаёт до моста по сети. Файловый канал даёт ему те же инструменты через папку «agent-channel» рядом с мостом; управлять ПК он так же не может.</div>
          <div class="row" style="margin-top:6px;gap:10px;align-items:center;flex-wrap:nowrap">
            <label class="chip" style="flex:none"><input type="checkbox" checked={!!AG.fileChannel?.on} onchange={(e) => fileChannel((e.currentTarget as HTMLInputElement).checked)} /> канал для агента без сети</label>
            {#each [fcState(AG.fileChannel)] as [t, c]}<span class="small fcline" style={c === 'ok' ? 'color:var(--ok)' : c === 'bad' ? 'color:var(--bad)' : ''} class:muted={c === 'muted'} title={t}>{t}</span>{/each}
          </div>
          <div class="small muted" style="margin-top:4px">Агенту: подключить папку моста и вызывать <code>./efc --channel agent-channel call bridge_peers</code> (клиент <code>efc</code> лежит в папке канала).</div>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .ag-img { position: relative; display: inline-block; margin: 4px 0 6px 14px; padding: 0; border: 1px solid var(--line, #333); border-radius: 6px; background: repeating-conic-gradient(#2a2f3a 0% 25%, #1f232c 0% 50%) 50% / 16px 16px; cursor: zoom-in; overflow: hidden; }
  .ag-img img { display: block; height: 96px; width: auto; max-width: 240px; object-fit: contain; }
  .ag-img span { display: none; }
  .ag-img:global(.nofile) img { display: none; }
  .ag-img:global(.nofile) span { display: flex; align-items: center; justify-content: center; width: 96px; height: 96px; font-size: 11px; opacity: .6; text-align: center; }
  .ag-strip { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }
  .ag-strip .ag-img { margin: 0; }
  .fcline { flex: 1; min-width: 0; height: 1.4em; line-height: 1.4em; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
</style>
