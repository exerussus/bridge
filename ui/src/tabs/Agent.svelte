<script lang="ts">
  import { ui, toast, titleOf, badgeCount } from '../lib/state.svelte';
  import { api } from '../lib/api';
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
  let cfg = $state<any>({ mode: 'off', allowFrom: [], workDir: '', claude: '', permissionMode: 'auto', maxMinutes: 60, maxParallel: 3, maxBudgetUsd: '' });
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
      cfg = { mode: st.mode, allowFrom: [...st.allowFrom], workDir: st.workDir, claude: st.claude, permissionMode: st.permissionMode || 'auto', maxMinutes: st.maxMinutes, maxParallel: st.maxParallel, maxBudgetUsd: st.maxBudgetUsd > 0 ? st.maxBudgetUsd : '' };
    }
    if (cur) show(cur, true);
    clearTimeout(timer);
    if (ui.tab === 'agent') timer = setTimeout(refresh, AG.threads.some((t: any) => live(t.status)) ? 1500 : 5000);
  }
  $effect(() => { refresh(); return () => clearTimeout(timer); });
  // кому поручать: этот ПК (если агент здесь включён) и соседи, которые принимают
  function cands(a: any): any[] {
    const l = a.local && a.local.mode !== 'off' ? [{ name: a.local.name, title: 'Этот ПК', online: true, agent: a.local.mode, local: true }] : [];
    return l.concat(a.peers.filter((p: any) => p.agent));
  }
  const can = $derived(AG ? cands(AG) : []);
  const isLocal = (n: string) => !!AG?.local && AG.local.name === n;
  const who = (n: string) => isLocal(n) ? 'этому ПК' : titleOf(n);

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
    const body = { mode: cfg.mode, allowFrom: cfg.allowFrom, workDir: String(cfg.workDir).trim(), claude: String(cfg.claude).trim(), permissionMode: cfg.permissionMode, maxMinutes: +cfg.maxMinutes || 60, maxParallel: +cfg.maxParallel || 3, maxBudgetUsd: +String(cfg.maxBudgetUsd).replace(',', '.') || 0 };
    try { await api('POST', 'agent/settings', body); dirty = false; toast('Сохранено', 'ok'); (document.activeElement as HTMLElement)?.blur(); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function exchange(p: string) { try { await api('POST', 'agent/exchange?peer=' + encodeURIComponent(p)); toast('Обменная папка открыта для ' + titleOf(p), 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); } }
  async function copyText(t: string) { try { await navigator.clipboard.writeText(t); toast('Скопировано', 'ok'); } catch { toast('Не удалось скопировать — выделите вручную', 'bad'); } }
  const pill = (st: string) => AGST[st] || [st, ''];
  const lines = (x: string) => String(x || '').split('\n');
</script>

<div class="rooms">
  <div>
    <div class="card"><h3>Поручить</h3>
      <label class="f">Кому<select bind:value={peer}>{#each can as p (p.name)}<option value={p.name}>{(p.local ? 'Этот ПК' : p.title || p.name) + (p.online ? '' : ' (нет связи)') + ' — ' + (p.agent === 'auto' ? 'Claude запустится сам' : 'ждёт сессию Claude')}</option>{/each}</select></label>
      <label class="f">Что сделать<textarea bind:value={prompt} rows="5" placeholder="Например: прогони тесты базиса на стенде, положи лог и скриншоты падений в обменную папку, итогом — список упавших."></textarea></label>
      <div class="row"><button class="b pri" disabled={!can.length} onclick={ask}>Поручить</button><span class="small muted">{AG && !can.length ? 'Ни один сосед не принимает поручения от этого ПК: на его панели «Агенты» → режим и «кто может поручать».' : ''}</span></div>
    </div>
    <div class="card" id="agThreads" style="margin-top:14px"><h3>Нити</h3>
      {#if err}<div class="pill bad">{err}</div>
      {:else if AG}
        {#each AG.threads as t (t.id)}
          <div class="chan" class:on={t.id === cur} style="cursor:pointer" role="button" tabindex="0" onclick={() => show(t.id)} onkeydown={() => {}}>
            <div class="row"><b style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:62%">{t.title || t.id}</b><div class="grow"></div><span class="pill {pill(t.status)[1]}">{pill(t.status)[0]}</span></div>
            <div class="small muted">{t.local ? '⟲ на этом ПК' : t.role === 'out' ? '→ ' + titleOf(t.peer) : '← от ' + titleOf(t.peer)} · {ago(t.updated)}{t.cost ? ' · $' + t.cost.toFixed(2) : ''}</div>
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
        <div class="small muted" style="margin-bottom:8px">{th.local ? 'поручено этому ПК' : th.role === 'out' ? 'поручено ' + titleOf(th.peer) : 'поручение от ' + titleOf(th.peer)}{th.cost ? ' · $' + th.cost.toFixed(2) : ''}{#if th.exchangePath} · обмен: <span class="mono">{th.exchangePath}</span>{/if}</div>
        {#if th.role === 'in' && th.status === 'queued' && !th.taken}
          <div class="banner" style="margin-bottom:10px"><span>✦</span><div class="grow small">Ждёт сессию Claude на этом ПК: откройте Claude (Desktop → Code) с подключёнными инструментами моста и скажите «возьми задачу из моста».</div></div>
        {/if}
        {#if busy}<AgentAsk {th} onanswered={() => show(cur!, true)} />{/if}
        <div class="log ag" bind:this={log}>
          {#each events as e, i (i)}
            {#if e.kind === 'prompt'}<div class="ag-p"><span class="small muted">поручение · {ago(e.t)}</span><div>{#each lines(e.text) as l, j}{#if j}<br />{/if}{l}{/each}</div></div>
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
            <label class="chip"><input type="radio" value="off" bind:group={cfg.mode} /> выключен</label>
            <label class="chip"><input type="radio" value="auto" bind:group={cfg.mode} /> сам запускает Claude</label>
            <label class="chip"><input type="radio" value="manual" bind:group={cfg.mode} /> ждёт сессию Claude</label>
          </div>
          <div class="f">Кто может поручать<div class="row">
            <label class="chip"><input type="checkbox" value="*" bind:group={cfg.allowFrom} /> все соседи</label>
            {#each Object.entries(ui.ov.groups || {}) as [g, v]}<label class="chip"><input type="checkbox" value={'@' + g} bind:group={cfg.allowFrom} /> 👥 {(v as any).title || g}</label>{/each}
            {#each AG.peers as p (p.name)}<label class="chip"><input type="checkbox" value={p.name} bind:group={cfg.allowFrom} /> {p.title || p.name}</label>{/each}
          </div></div>
          <label class="f">Рабочая папка Claude<input bind:value={cfg.workDir} placeholder="пусто — папка «агент» рядом с мостом" /></label>
          <div class:hide={cfg.mode !== 'auto'}>
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
        </div>
      </div>
    {/if}
  </div>
</div>
