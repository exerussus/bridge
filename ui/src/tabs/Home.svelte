<script lang="ts">
  // «Обзор» (3.21): плитки — соседи на связи, путь связи, передачи, сборки, каналы, записи, последние файлы.
  // Сверху — только то, что ждёт решения.
  import { ui, setTab, toast, load, peerTitle, titleOf } from '../lib/state.svelte';
  import { api, act } from '../lib/api';
  import { ago, size } from '../lib/util';
  import { NS, relayLine, netLine } from '../lib/netstat.svelte';
  import { V, CH, R, roomKey, rosterOf, roomJoin } from '../voice/engine.svelte';
  import Pill from '../components/Pill.svelte';
  import Gi from '../components/Gi.svelte';

  const ov = $derived(ui.ov);
  const live = $derived(ov.peers.filter((p: any) => p.enabled && !p.invite));
  const onl = $derived(live.filter((p: any) => p.online));
  const off = $derived(live.filter((p: any) => !p.online));
  const HUE = ['56,225,240', '93,224,142', '255,181,71', '160,140,255', '255,120,180'];
  const hue = (n: string) => HUE[[...n].reduce((a, c) => a + c.charCodeAt(0), 0) % HUE.length];
  function kindOf(hp: string): string {
    if (!hp) return '';
    if (hp === 'сервер') return 'свой сервер';
    const o = hp.replace(/:\d+$/, '').split('.').map(Number);
    return o[0] === 26 ? 'Radmin' : o[0] === 10 && o[1] === 77 ? 'своя сеть' : (o[0] === 192 && o[1] === 168) || o[0] === 10 || (o[0] === 172 && o[1] >= 16 && o[1] < 32) ? 'локальная сеть' : 'напрямую';
  }
  const rl = $derived((void NS.at, relayLine()));
  const nl = $derived((void NS.at, netLine()));
  const radmin = $derived(onl.filter((p: any) => kindOf(p.via) === 'Radmin').length);

  // записи
  let recent = $state<any[]>([]);
  async function loadRecs() {
    const all: any[] = [];
    for (const s of ui.ov.shares) { try { (await api('GET', 'records?share=' + encodeURIComponent(s.id))).forEach((r: any) => all.push(r)); } catch {} }
    all.sort((a, b) => +new Date(b.updated) - +new Date(a.updated));
    recent = all.slice(0, 4);
  }
  let lastRecs = 0;
  $effect(() => { void ui.ov; if (Date.now() - lastRecs > 15000) { lastRecs = Date.now(); loadRecs(); } });
  function openRecord(share: string, id: string) { ui.recShare = share; ui.curRecord = id; setTab('records'); }

  // сборки
  let B = $state<any>(null);
  $effect(() => {
    let t: any = 0, alive = true;
    const tick = async () => { try { B = await api('GET', 'builds'); } catch {} if (alive) t = setTimeout(tick, 8000); };
    tick();
    return () => { alive = false; clearTimeout(t); };
  });
  const builds = $derived.by(() => {
    if (!B) return [];
    const L: any[] = [];
    for (const r of B.received.slice(0, 2)) L.push({ mine: false, name: r.name, n: (r.lastOk || r).n, sub: 'от ' + titleOf(r.peer) + ' · ' + ago((r.lastOk || r).at), r, running: r.running });
    for (const p of (B.presets || []).slice(0, 3 - L.length)) {
      const D = p.delivery || [];
      L.push({ mine: true, name: p.name, n: p.latest?.n, sub: p.running ? 'запущена у вас' : D.length ? 'у ' + D.filter((d: any) => d.state === 'unpacked').length + ' из ' + D.length + (D.some((d: any) => d.state === 'waiting') ? ' · ждёт' : '') : 'моя', p, running: p.running });
    }
    return L;
  });
  async function runBuild(b: any) {
    try {
      if (b.mine) await api('POST', b.running ? 'builds/stop' : 'builds/run', { preset: b.p.id });
      else if (b.running) await api('POST', 'builds/stop', { peer: b.r.peer, name: b.r.name });
      else { setTab('builds'); return; } // присланное — с подтверждением на вкладке «Сборки»
      B = await api('GET', 'builds');
    } catch (e: any) { toast(e.message, 'bad'); }
  }

  // каналы
  const chans = $derived.by(() => { void V.v; return (CH.all || []).slice(0, 3).map((c: any) => ({ key: roomKey(c), title: c.title, who: rosterOf(roomKey(c)).map(x => titleOf(x.name)), mine: R.key === roomKey(c) })); });

  // последние файлы — пакетами
  const files = $derived.by(() => {
    const L: any[] = [];
    for (const e of ov.mailbox.recent || []) {
      if (e.kind !== 'sent' && e.kind !== 'received') continue;
      const t = e.path.split('/')[0], last = L[L.length - 1];
      if (last && last.kind === e.kind && last.peer === e.peer && last.key === t) { last.n++; last.bytes += e.size; continue; }
      L.push({ kind: e.kind, peer: e.peer, top: t === '_builds' || t === '_сборки' ? 'сборка «' + (e.path.split('/')[1] || '') + '»' : t, key: t, n: 1, bytes: e.size, time: e.time });
      if (L.length >= 4) break;
    }
    return L;
  });
  const hhmm = (t: string) => new Date(t).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });

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
  const conflicts = $derived(ov.shares.reduce((a: number, s: any) => a + s.conflicts.length, 0));
  const rejected = $derived(ov.shares.reduce((a: number, s: any) => a + s.rejected, 0));
</script>

{#each ov.waiting as c (c.record)}
  <div class="card attn">
    <span class="av2" style="background:var(--warn-soft);color:var(--warn)"><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8.5" /><path d="M12 7.5V12l3 2" /></svg></span>
    <div class="grow" style="min-width:240px"><b>{titleOf(c.from)} просит выполнить команду на этом ПК</b>
      <div class="small muted"><span class="mono" style="color:var(--text)">{c.runner} {c.args}</span>{c.note ? ' — ' + c.note : ''} · {c.record} · {ago(c.since)}</div></div>
    <button class="b" onclick={() => decide(c, false)}>Отказать</button>
    <button class="b pri" onclick={() => decide(c, true)}>Выполнить</button>
  </div>
{/each}
{#if conflicts + rejected}
  <button class="card attn" style="width:100%;text-align:left;cursor:pointer;color:inherit;font:inherit" onclick={() => setTab('shares')}>
    <span class="dot warn"></span><span class="grow">В пространствах: конфликтов файлов {conflicts} · отклонённых событий {rejected}</span><span class="more">Пространства ›</span>
  </button>
{/if}

<div class="tiles">
  <section class="tile2" style="flex:7 1 460px">
    <div class="th"><h3>Соседи</h3><button class="more" onclick={() => setTab('peers')}>Все соседи ›</button></div>
    <div class="row" style="align-items:baseline;gap:12px;margin:8px 0 6px">
      <span class="hero-n">{onl.length}</span><span style="font:300 22px var(--display)">на связи</span><span class="muted">из {live.length}</span>
    </div>
    {#each onl.slice(0, 4) as p (p.name)}
      <button class="li hb" onclick={() => { ui.peerOpen = p.name; ui.peersSub = 'list'; setTab('peers'); }}>
        <span class="av2" style="background:rgba({hue(p.name)},.12);color:rgb({hue(p.name)})">{(p.title || p.name).slice(0, 1).toUpperCase()}</span>
        <span class="grow" style="min-width:0"><b class="ell" style="display:block">{p.title || p.name}</b><span class="small muted ell" style="display:block">{kindOf(p.via)}{p.via && p.via !== 'сервер' ? ' · ' + p.via.replace(/:\d+$/, '') : ''}{p.filesVia ? ' · файлы через сервер' : ''}</span></span>
        <span class="pill plain okf">{p.latency != null ? p.latency + ' мс' : 'на связи'}</span>
      </button>
    {:else}<div class="small muted" style="padding:8px 0">{live.length ? 'Никого нет на связи' : 'Соседей пока нет — «Соседи» → «Пригласить»'}</div>{/each}
    {#if onl.length > 4}<div class="small muted">и ещё {onl.length - 4} на связи</div>{/if}
    {#if off.length}
      <button class="li hb" style="margin-top:auto;color:var(--muted)" onclick={() => { ui.peersSub = 'list'; setTab('peers'); }}>
        <span class="row" style="gap:0">{#each off.slice(0, 3) as p, i}<span class="av2" style="width:26px;height:26px;border-radius:9px;font-size:11px;background:var(--panel2);border:2px solid var(--panel);{i ? 'margin-left:-8px' : ''}">{(p.title || p.name).slice(0, 1).toUpperCase()}</span>{/each}</span>
        <span class="grow small ell">{off.map((p: any) => p.title || p.name).join(', ')} — не на связи</span><span class="more">почему ›</span>
      </button>
    {/if}
  </section>

  <section class="tile2" style="flex:5 1 320px">
    <div class="th"><h3>Путь связи</h3><button class="more" onclick={() => { ui.peersSub = 'net'; setTab('peers'); }}>Сеть ›</button></div>
    <div class="li"><span class="dot {rl.dot}"></span><span class="grow"><b style="font-weight:600">Свой сервер</b><span class="small muted" style="display:block">{NS.relay?.configured ? (rl.text + (NS.relay?.linkUp ? ' · связь мостов через сервер' : '')) : 'не подключён — без него всё работает напрямую'}</span></span><span class="small muted">{rl.sub}</span></div>
    <div class="li"><span class="dot {nl.dot}"></span><span class="grow"><b style="font-weight:600">Своя сеть</b><span class="small muted" style="display:block">MostNet{NS.net?.service?.vip ? ' · этот ПК ' + NS.net.service.vip : ''}</span></span><span class="small muted">{nl.sub}</span></div>
    <div class="li"><span class="dot {radmin ? 'ok' : ''}"></span><span class="grow"><b style="font-weight:600">Radmin</b><span class="small muted" style="display:block">{radmin ? 'через него связь с ' + radmin : 'сейчас не используется'}</span></span></div>
    <div class="li"><span class="dot {NS.relay?.files && NS.relay?.netUp ? 'ok' : ''}"></span><span class="grow"><b style="font-weight:600">Файлы через сервер</b><span class="small muted" style="display:block">{NS.relay?.files ? (NS.relay?.netUp ? 'включено · зашифрованы от ПК до ПК' : 'включено, но своя сеть не подключена') : 'выключено'}</span></span></div>
  </section>

  <section class="tile2" style="flex:4 1 280px">
    <div class="th"><h3>Передачи{ov.transfers?.length ? ' · ' + ov.transfers.length : ''}</h3><button class="more" onclick={() => setTab('send')}>Отправка ›</button></div>
    {#each (ov.transfers || []).slice(0, 3) as x}
      {@const pct = x.total > 0 ? Math.min(100, Math.round((x.done * 100) / x.total)) : null}
      <div style="padding:8px 0">
        <div class="row small" style="gap:8px"><b class="grow ell">{x.kind === 'up' || x.kind === 'serve' ? '↑' : '↓'} {x.path.split('/').pop()}</b><span class="mono muted">{pct ?? '…'}%</span></div>
        <div class="bar" style="margin:6px 0 5px"><i style="width:{pct ?? 100}%"></i></div>
        <div class="small muted ell">{x.kind === 'serve' ? titleOf(x.peer) + ' забирает' : x.kind === 'down' ? 'скачиваю у ' + titleOf(x.peer) : titleOf(x.peer)} · {size(x.speed)}/с{x.eta >= 0 ? ' · осталось ' + x.eta + ' с' : ''}</div>
      </div>
    {:else}<div class="small muted" style="padding:8px 0">Сейчас ничего не передаётся.</div>{/each}
  </section>

  <section class="tile2" style="flex:4 1 280px">
    <div class="th"><h3>Сборки</h3><button class="more" onclick={() => setTab('builds')}>Все ›</button></div>
    {#each builds as b (b.mine + b.name)}
      <div class="li">
        <span class="av2" style={b.mine ? 'background:var(--ok-soft);color:var(--ok)' : 'background:var(--accent-soft);color:var(--cyan)'}><Gi n="box" /></span>
        <span class="grow" style="min-width:0"><b class="ell" style="display:block">{b.name} {#if b.n}<span class="muted" style="font-weight:500">№{b.n}</span>{/if}</b><span class="small muted ell" style="display:block">{#if b.running}<span class="dot ok" style="margin-right:6px"></span>{/if}{b.sub}</span></span>
        {#if b.running}<button class="b sm danger" onclick={() => runBuild(b)}>Стоп</button>
        {:else if b.mine}<button class="b sm" onclick={() => runBuild(b)} disabled={!b.p.exe}>▶</button>
        {:else}<button class="b pri sm" onclick={() => runBuild(b)}>▶ Запуск</button>{/if}
      </div>
    {:else}<div class="small muted" style="padding:8px 0">Сборок пока нет.</div>{/each}
  </section>

  <section class="tile2" style="flex:4 1 280px">
    <div class="th"><h3>Каналы</h3><button class="more" onclick={() => setTab('voice')}>Все ›</button></div>
    {#each chans as c (c.key)}
      <div class="li"><span class="dot {c.who.length ? 'bad' : ''}"></span><span class="grow" style="min-width:0"><b class="ell" style="display:block">{c.title}</b><span class="small muted ell" style="display:block">{c.who.length ? 'в эфире: ' + c.who.join(', ') : 'никого'}</span></span>
        <button class="b sm" onclick={() => (c.mine ? setTab('voice') : roomJoin(c.key))}>{c.mine ? 'Вы здесь' : 'Войти'}</button></div>
    {:else}<div class="small muted" style="padding:8px 0">Каналов пока нет.</div>{/each}
  </section>

  <section class="tile2" style="flex:6 1 380px">
    <div class="th"><h3>Записи</h3><button class="more" onclick={() => setTab('records')}>Все записи ›</button></div>
    {#each recent as r (r.share + r.id)}
      <button class="li hb" onclick={() => openRecord(r.share, r.id)}><span class="grow" style="min-width:0"><b class="ell" style="display:block;font-weight:600">{r.title || r.id}</b><span class="small muted ell" style="display:block">{r.share}{r.assignee ? ' · ' + peerTitle(r.assignee) : ''} · {ago(r.updated)}</span></span><Pill s={r.status} /></button>
    {:else}<div class="small muted" style="padding:8px 0">Записей пока нет.</div>{/each}
  </section>

  <section class="tile2" style="flex:6 1 380px">
    <div class="th"><h3>Последние файлы</h3><button class="more" onclick={() => setTab('send')}>Все ›</button></div>
    {#each files as f}
      <div class="li"><span class="muted">{f.kind === 'sent' ? '↑' : '↓'}</span><span class="grow" style="min-width:0"><b class="ell" style="display:block;font-weight:600">{f.top}{#if f.n > 1}<span class="muted" style="font-weight:500">&nbsp;· {f.n} файл.</span>{/if}</b><span class="small muted ell" style="display:block">{f.kind === 'sent' ? '→ ' + titleOf(f.peer) : 'от ' + titleOf(f.peer)} · {size(f.bytes)}</span></span><span class="small muted">{hhmm(f.time)}</span></div>
    {:else}<div class="small muted" style="padding:8px 0">Пока ничего не передавали.</div>{/each}
  </section>
</div>

<style>
  .attn { display: flex; align-items: center; gap: 16px; padding: 14px 16px 14px 18px; flex-wrap: wrap; margin-bottom: 16px; border-color: rgba(255, 181, 71, 0.35); background: linear-gradient(90deg, var(--warn-soft), var(--panel) 60%); }
  .hb { width: 100%; border: 0; border-top: 1px solid var(--line); background: none; color: inherit; font: inherit; text-align: left; cursor: pointer; }
  .hb:first-of-type { border-top: 0; }
  .hb:hover { background: rgba(160, 170, 255, 0.03); }
</style>
