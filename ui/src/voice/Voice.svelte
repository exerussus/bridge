<script lang="ts">
  import { artOf, artImg } from '../lib/chanart.svelte';
  const roomArt = () => (void V.v, artOf(roomDef(R.key)));
  import { dropSend } from '../lib/dropsend';
  import TileArt from '../components/TileArt.svelte';
  import { tileOf, tileImgUrl, stImgUrl, tileVars, tileCls } from '../lib/tile.svelte';
  import { statusHover } from '../lib/statuspop';
  import { openStatusMenu } from '../lib/statusmenu';
  // Каналы: слева список каналов, в центре канал (вкладки «Участники», «Показы», «Доска») и панель управления внизу,
  // справа чат. Кнопки панели не исчезают и не двигаются: недоступное — серое.
  import { ui, SET, setSet, titleOf, me, openModal } from '../lib/state.svelte';
  import { popover } from '../lib/pop';
  import { keyName } from '../lib/avatar';
  import { goSettings } from '../lib/nav';
  import { CHT } from '../lib/chat.svelte';
  import {
    R, V, VV, SPK, PINGED, GREET, INV, CH, roomKey, roomDef, rosterOf, myState, isChannel, connState, roomJoin, roomLeave, toggleMic, toggleDeaf, setPtt,
    shareStart, ping, forceMute, askScreen, watchAllOf, dropInvite, loadChannels,
  } from './engine.svelte';
  import { Board } from './board';
  import Gi from '../components/Gi.svelte';
  import Av from '../components/Av.svelte';
  import ChatBox from '../components/ChatBox.svelte';
  import Streams from './Streams.svelte';
  import Seg from '../components/Seg.svelte';
  import ChanEdit from './ChanEdit.svelte';
  import MicPop from './pops/MicPop.svelte';
  import OutPop from './pops/OutPop.svelte';
  import VolPop from './pops/VolPop.svelte';
  import CfgPop from './pops/CfgPop.svelte';
  import InvitePop from './pops/InvitePop.svelte';
  import CallPop from './pops/CallPop.svelte';

  const plural = (n: number, a: string, b: string, c: string) => { n = Math.abs(n) % 100; const m = n % 10; return n > 10 && n < 20 ? c : m > 1 && m < 5 ? b : m === 1 ? a : c; };
  const IND = (st: any) => {
    if (!st) return [] as { t: string; cls: string; tip: string }[];
    const b: { t: string; cls: string; tip: string }[] = [];
    if (st.dev) b.push({ t: '📱 ' + st.dev, cls: '', tip: 'зашёл с телефона' });
    if (st.voice === false) b.push(st.dev ? { t: '🎧 только слушает', cls: 'off', tip: 'с телефона по обычному адресу — говорить можно только по HTTPS' } : { t: '🎙 нет микрофона', cls: 'off', tip: 'микрофон не подключился' });
    else if (st.deaf) b.push({ t: '🔇 не слышит', cls: 'off', tip: 'выключил звук и микрофон' });
    else if (st.fm) b.push({ t: '🔇 заглушён', cls: 'off', tip: 'заглушил ' + titleOf(st.fm) });
    else if (!st.mic) b.push({ t: '🎙 выкл', cls: 'off', tip: 'микрофон выключен' });
    if (st.streams && st.streams.length) b.push({ t: '🖥 показывает' + (st.streams.length > 1 ? ' ' + st.streams.length : ''), cls: 'live', tip: st.streams.map((s: any) => s.label).join(', ') });
    const box = ROSTER_OF();
    const wl = (st.watching || []).filter((id: string) => Object.values(box).some((v: any) => (v.st.streams || []).some((x: any) => x.id === id)) || R.mine[id]);
    if (wl.length) {
      const whose = wl.map((id: string) => { for (const [n, v] of Object.entries(box) as [string, any][]) { const s = (v.st.streams || []).find((x: any) => x.id === id); if (s) return n === me() ? 'вас' : titleOf(n); } return null; }).filter(Boolean);
      const u = [...new Set(whose)].join(', ');
      b.push({ t: '👁 смотрит' + (u ? ' ' + u : ''), cls: '', tip: 'смотрит показ: ' + u });
    }
    return b;
  };
  import { ROSTER } from './engine.svelte';
  const ROSTER_OF = () => (R.key && ROSTER[R.key]) || {};
  const shortInd = (st: any) => (!st ? '' : (st.dev ? '📱' : '') + (st.voice === false ? '🎙✕' : st.deaf ? '🔇' : !st.mic ? '🎙✕' : '🎙') + (st.streams && st.streams.length ? '🖥' : ''));

  // статус: из состояния канала (приходит сразу), у старых версий — из плитки
  const tileStatus = (tl: any) => (tl && tl.style && (tl.style.stText || tl.style.stColor || tl.stImg || tl.style.stSilent) ? { text: tl.style.stText || '', color: tl.style.stColor || '', silent: !!tl.style.stSilent, img: tl.stImg || '' } : null);
  // ── снимок для отрисовки (движок говорит о переменах счётчиком V.v) ──
  const S = $derived.by(() => {
    void V.v; void ui.ov; void CHT.list;
    const key = R.key;
    const chans = CH.all.map(c => {
      const k = roomKey(c), inIt = key === k;
      const people = (inIt ? [{ name: me(), st: myState() }] : []).concat(rosterOf(k));
      return { c, k, inIt, people };
    });
    if (!key) return { key, chans, people: [] as any[], room: null as any };
    const def = roomDef(key);
    const people = rosterOf(key).map(p => ({ ...p, ...connState(p.name) }));
    const nstreams = people.reduce((a, p) => a + (p.st.streams || []).length, 0) + Object.keys(R.mine).length;
    const lost = people.length > 0 && people.every(p => p.bad);
    const you = R.forced ? ['df', '🔇 Вас заглушил ' + titleOf(R.forced) + ' — нажмите «Микрофон», когда вернётесь'] : lost ? ['ls', '⚠ Нет связи с участниками — соединяюсь заново…'] : !R.voice ? ['df', '🎙 Микрофон не подключился — вас не слышно («Канал» → «Подключить микрофон»)'] : R.deaf ? ['df', '🔇 У вас выключен звук: вы никого не слышите, и вас не слышно'] : !R.mic ? ['df', '🎙 Микрофон выключен — вас не слышно'] : R.ptt ? ['ok', '🎙 Рация: говорите, пока держите ' + keyName(SET.pttKey)] : ['ok', '🎙 Вы в разговоре'];
    const unread = (CHT.list.find(c => c.conv === 'ch:' + key) || {}).unread || 0;
    return {
      key, chans, people,
      room: { title: R.title, info: (def && def.owner && def.owner !== me() ? 'у ' + titleOf(def.owner) + ' · ' : '') + 'участники: ' + (people.length + 1), n: people.length + 1, nstreams, lost, you, chan: isChannel(key), unread,
        voice: R.voice, mic: R.mic, deaf: R.deaf, ptt: R.ptt, pttDown: R.pttDown, forced: R.forced, nmine: Object.keys(R.mine).length, me: myState() },
    };
  });
  // красный — только выключенный микрофон или звук; показ экрана — синяя рамка и значок экрана справа вверху
  const stCls = (st: any, bad: boolean, mine: boolean) => { if (!st) return ''; if (!mine && bad) return 'st-lost'; if (st.deaf) return 'st-deaf'; if (!st.mic) return 'st-muted'; return ''; };
  const stLbl: Record<string, string> = { 'st-lost': 'нет связи', 'st-deaf': 'не слышит' };
  function openShows(n: string, mine: boolean) { if (mine) { VV.view = 'shows'; VV.sub = 'list'; return; } watchAllOf(n); setTimeout(() => { const st = rosterOf(R.key).find(p => p.name === n)?.st; const id = st?.streams?.[0]?.id; if (id) document.querySelector('.stream[data-stream="' + CSS.escape(id) + '"]')?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }); }, 120); }

  // ── раскладка: колонки, их ширина (тянуть за границу), высота до низа окна ──
  let vx = $state<HTMLElement>();
  let rail = $state<HTMLElement>();
  let chat = $state<HTMLElement>();
  let wide = $state(innerWidth > 1180);
  let wideBoard = $state(innerWidth >= 1600);
  const chatFold = $derived(!!SET.chatFold || !S.key || (VV.view === 'board' && !wideBoard && !VV.chatPinned));
  let splitL = $state(0), splitR = $state(0);
  function place() {
    if (!vx || !rail) return;
    const vr = vx.getBoundingClientRect();
    splitL = rail.getBoundingClientRect().right - vr.left + 7;
    if (chat && !chatFold) splitR = chat.getBoundingClientRect().left - vr.left - 7;
  }
  function fit() {
    if (!vx) return;
    if (ui.tab !== 'voice' || innerWidth <= 1180) { vx.style.height = ''; return; }
    const top = vx.getBoundingClientRect().top + scrollY;
    const h = Math.max(560, innerHeight - top - 14);
    vx.style.height = h + 'px';
    const over = document.documentElement.scrollHeight - innerHeight;
    if (over > 0 && h - over >= 560) vx.style.height = h - over + 'px';
  }
  $effect(() => {
    void ui.tab; void SET.railW; void SET.chatW; void SET.railFold; void chatFold; void S.key;
    requestAnimationFrame(() => { fit(); place(); });
  });
  $effect(() => {
    const r = () => { wide = innerWidth > 1180; wideBoard = innerWidth >= 1600; fit(); place(); };
    addEventListener('resize', r);
    return () => removeEventListener('resize', r);
  });
  function drag(e: PointerEvent, key: 'railW' | 'chatW') {
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId); el.classList.add('on'); document.body.classList.add('vsplitting');
    const side = key === 'railW' ? 1 : -1;
    const x0 = e.clientX, w0 = (key === 'railW' ? rail! : chat!).getBoundingClientRect().width;
    const mv = (ev: PointerEvent) => {
      const total = vx!.getBoundingClientRect().width, other = key === 'railW' ? (SET.chatFold ? 0 : SET.chatW || 320) : SET.railFold ? 64 : SET.railW || 240;
      SET[key] = Math.max(key === 'railW' ? 180 : 260, Math.min(total - other - 420, w0 + (ev.clientX - x0) * side));
      requestAnimationFrame(place);
    };
    const upf = () => { el.classList.remove('on'); document.body.classList.remove('vsplitting'); el.removeEventListener('pointermove', mv); el.removeEventListener('pointerup', upf); setSet(key, SET[key]); };
    el.addEventListener('pointermove', mv); el.addEventListener('pointerup', upf);
  }
  function chatToggle() {
    if (VV.view === 'board' && !wideBoard) { VV.chatPinned = chatFold; if (chatFold) setSet('chatFold', false); }
    else setSet('chatFold', !chatFold);
  }

  // ── доска ──
  let boardHost = $state<HTMLElement>();
  let boardOn = $state(false);
  $effect(() => { if (boardHost) Board.host(boardHost); });
  $effect(() => Board.onChange(() => (boardOn = Board.enabled)));
  $effect(() => { Board.room(S.room && S.room.chan ? S.key : null); });
  $effect(() => { Board.visible(ui.tab === 'voice' && VV.view === 'board' && !!S.room?.chan); });
  $effect(() => { if (VV.view === 'board' && S.key && !S.room?.chan) VV.view = 'people'; });

  /** Вспышка плитки при пинге: перезапуск анимации на каждое новое значение. */
  function flash(node: HTMLElement, v: number) {
    let last = v, t: any = 0;
    return {
      update(n: number) {
        if (!n || n === last) return;
        last = n;
        node.classList.remove('pinged'); void node.offsetWidth; node.classList.add('pinged');
        clearTimeout(t); t = setTimeout(() => node.classList.remove('pinged'), 1200);
      },
    };
  }

  // ── попапы панели ──
  const pop = (e: Event, kind: string, c: any, p: any = {}) => { e.stopPropagation(); const b = (e.currentTarget as HTMLElement).closest('button')!; if ((b as HTMLButtonElement).disabled) return; popover(b, kind, c, p); };
  $effect(() => { if (ui.tab === 'voice') loadChannels(); });
</script>

<div id="roomInvites">
  {#each INV as m (m.id)}
    <div class="banner">
      <span style="font-size:22px"><Gi n={m.media === 'screen' ? 'screen' : 'phone'} /></span>
      <div class="grow"><b>{m.text}</b><div class="small muted">{titleOf(m.from)} · {new Date((m.ts || Date.now() / 1000) * 1000).toLocaleTimeString()}</div></div>
      <button class="b pri" onclick={() => { dropInvite(m.id); m.accept(); }}>Принять</button>
      <button class="b danger" onclick={() => { dropInvite(m.id); m.decline(); }}>Отклонить</button>
    </div>
  {/each}
</div>
<div class="vx" bind:this={vx} class:rail-fold={!!SET.railFold} class:chat-fold={chatFold} style="--railW:{Math.round(SET.railW || 240)}px;--chatW:{Math.round(SET.chatW || 320)}px">
  <div class="vsplit" title="Потяните, чтобы изменить ширину" style="left:{splitL}px;display:{SET.railFold || !wide ? 'none' : ''}" role="separator" onpointerdown={e => drag(e, 'railW')} ondblclick={() => setSet('railW', 240)}></div>
  <div class="vsplit" title="Потяните, чтобы изменить ширину" style="left:{splitR}px;display:{chatFold || !wide ? 'none' : ''}" role="separator" onpointerdown={e => drag(e, 'chatW')} ondblclick={() => setSet('chatW', 320)}></div>
  <div class="card vrail" bind:this={rail} role="complementary" aria-label="Каналы">
    <div class="vrow"><span class="cap rfull">Каналы</span><div class="grow rfull"></div>
      <button class="b sm ib rfull" title="Новый канал" onclick={() => openModal(ChanEdit, { id: null })}><Gi n="plus" /></button>
      <button class="b sm ib" title="Свернуть или развернуть список" onclick={() => setSet('railFold', !SET.railFold)}><Gi n="railL" /></button></div>
    <div id="chanList">
      {#each S.chans as ch (ch.k)}
        {#if SET.railFold}
          {@const art = artOf(ch.c)}
          <button class="chanmini" class:on={ch.inIt} title={ch.c.title + (ch.people.length ? ' · ' + ch.people.length + ' в канале' : '')} onclick={() => roomJoin(ch.k)}>{#if art && art.icon}<img class="cicon" alt="" src={artImg(ch.c, 'icon', art.icon)} />{:else}{ch.c.title.slice(0, 1).toUpperCase()}{/if}{#if ch.people.length && !ch.inIt}<b>{ch.people.length}</b>{/if}</button>
        {:else}
          {@const art = artOf(ch.c)}
          <div class="chan" class:on={ch.inIt} class:cart={!!art}>
            {#if art}<TileArt st={art.style} url={artImg(ch.c, 'back', art.back)} />{/if}
            <div class="row" style="gap:8px">{#if art && art.icon}<img class="cicon" alt="" src={artImg(ch.c, 'icon', art.icon)} />{/if}<b class="cname">{ch.c.title}</b>{#if ch.c.owner !== me()}<span class="small muted">у {titleOf(ch.c.owner)}</span>{/if}<div class="grow"></div>
              <span class="chbtns">{#if ch.inIt}<span class="pill ok">вы здесь</span>{:else}<button class="b sm pri" onclick={() => roomJoin(ch.k)}>Войти</button><button class="b sm ib" title="Войти тихо — без звука входа у остальных" onclick={() => roomJoin(ch.k, true)}><Gi n="quiet" /></button>{/if}</span></div>
            <div class="row cwho">
              {#if ch.people.length}
                {#each ch.people.slice(0, 6) as p (p.name)}<span class="cav" title={(p.name === me() ? 'Вы' : titleOf(p.name)) + ' ' + shortInd(p.st)}><Av n={p.name} title={p.name === me() ? me() : titleOf(p.name)} /></span>{/each}
                {#if ch.people.length > 6}<span class="small muted">+{ch.people.length - 6}</span>{/if}
                <span class="small muted">{ch.people.length} {plural(ch.people.length, 'участник', 'участника', 'участников')}</span>
              {:else}<span class="small muted">никого</span>{/if}
              {#if ch.c.owner === me()}<div class="grow"></div><a href="#chan" class="small" title="Настроить канал" onclick={e => { e.preventDefault(); openModal(ChanEdit, { id: ch.c.id }); }}><Gi n="gear" /></a>{/if}
            </div>
          </div>
        {/if}
      {:else}
        {#if !SET.railFold}<div class="empty">Каналов пока нет. Создайте — например «Семья» для группы или «Общий» для всех соседей. Соседи увидят канал у себя сами.</div>{/if}
      {/each}
    </div>
    <div class="grow"></div>
    <div class="vrfoot rfull"><span class="cap">Без канала</span><button class="b" style="width:100%" onclick={e => popover(e.currentTarget as Element, 'call', CallPop)}><Gi n="call" /> Позвонить…</button></div>
  </div>
  <section class="card vroom">
    {#if !S.room}
      <div class="vidle"><h3>Голос, экран и доска</h3>
        <div class="small muted">Зайдите в канал слева — вы сразу в разговоре со всеми, кто там. Кнопка с перечёркнутым колокольчиком — войти тихо, без звука входа у остальных.<br />
          Показ экрана идёт в канал: остальные видят, что вы показываете, и включают просмотр сами. Доска — общая для всех в канале: рисунки, картинки, PDF, заметки.<br />
          Звук и видео идут напрямую между ПК (локальная сеть или Radmin), без серверов.</div></div>
    {/if}
    <div class="vcall" id="roomCall" class:hide={!S.room} class:call-deaf={S.room && S.room.voice && S.room.deaf} class:call-lost={S.room?.lost}>
      {#if S.room}
        {@const rm = S.room}
        <header class="vrhead">
          {#if roomArt()?.back}<div class="vrback"><TileArt st={roomArt()!.style} url={artImg(roomDef(R.key), 'back', roomArt()!.back)} /></div>{/if}
          <div class="vtitle">{#if roomArt()?.icon}<img class="cicon big" alt="" src={artImg(roomDef(R.key), 'icon', roomArt()!.icon)} />{/if}<h2>{rm.title}</h2><span class="small muted">{rm.info}</span></div>
          <div class="grow"></div>
          <div class="vtabs" role="tablist" aria-label="Вид канала">
            <button class:on={VV.view === 'people'} role="tab" onclick={() => (VV.view = 'people')}><Gi n="people" /> Участники <span class="cnt">{rm.n}</span></button>
            <button class:on={VV.view === 'shows'} role="tab" onclick={() => (VV.view = 'shows')}><Gi n="screen" /> Показы <span class="cnt red" class:hide={!rm.nstreams}>{rm.nstreams}</span></button>
            <button class:on={VV.view === 'board'} role="tab" disabled={!rm.chan} onclick={() => (VV.view = 'board')}><Gi n="board" /> Доска <span class="cnt" style="opacity:{boardOn ? 1 : 0.5}">{boardOn ? 'вкл' : 'выкл'}</span></button>
          </div>
          <button class="b sm" onclick={e => popover(e.currentTarget as Element, 'invite', InvitePop)}><Gi n="invite" /> Позвать</button>
        </header>
        <div class="youline {rm.you[0]}">{rm.you[1]}</div>
        <div class="vview" data-vv="people" class:hide={VV.view !== 'people'}>
          <div class="call">
            {#each [{ name: me(), st: rm.me, cs: '', bad: false, mine: true }, ...S.people.map(p => ({ ...p, mine: false }))] as t (t.name)}
              {@const cls = stCls(t.st, t.bad, t.mine)}
              {@const streams = (t.st && t.st.streams) || []}
              {@const ind = IND(t.st)}
              {@const tl = tileOf(t.name)}
              {@const sts = t.st && 'sts' in t.st ? t.st.sts : tileStatus(tl)}
              <div class="tile {cls} {tileCls(tl?.style)}" style={tileVars(tl?.style)} class:speaking={SPK[t.mine ? '__me' : t.name]} class:sharing={streams.length > 0} class:greeting={GREET[t.name]} use:flash={PINGED[t.name] || 0} use:dropSend={t.mine ? null : t.name} role="group" aria-label={t.mine ? "Вы" : titleOf(t.name)} oncontextmenu={t.mine ? openStatusMenu : undefined} use:statusHover={sts ? { name: t.name, text: sts.text, color: sts.color, img: sts.img ? stImgUrl(t.name, sts.img) : '' } : null}>
                <TileArt st={tl?.style} url={tl ? tileImgUrl(t.name, tl.img) : ''} />
                {#if streams.length}<button class="tshare" title={(t.mine ? 'Ваш показ: ' : 'Смотреть показ: ') + streams.map((x: any) => x.label).join(', ')} onclick={() => openShows(t.name, t.mine)}><Gi n="screen" />{#if streams.length > 1}<b>{streams.length}</b>{/if}</button>{/if}
                {#if stLbl[cls]}<span class="stl">{stLbl[cls]}</span>{/if}
                {#if t.mine}
                  <div class="av me" title="Оформить свою плитку: картинка, маска, аватарка, цвета. Статус — правая кнопка по плитке" role="button" tabindex="0" onclick={() => goSettings('tile')} onkeydown={e => { if (e.key === 'Enter') goSettings('tile'); }}><Av n={me()} title={me()} /></div>
                {:else}
                  <div class="av"><Av n={t.name} title={titleOf(t.name)} /></div>
                {/if}
                <b>{#if sts && (sts.img || sts.color)}<i class="stdot" style={sts.img ? '' : 'background:' + sts.color}>{#if sts.img}<img src={stImgUrl(t.name, sts.img)} alt="" />{/if}</i>{/if}{t.mine ? 'Вы' : titleOf(t.name)}{#if sts?.silent}<span class="stsil" title="Полная тишина: звуки приложения у него выключены, пинги не проходят">🔕</span>{/if}</b>
                <div class="inds">{#each ind as x}<span class="ind {x.cls}" title={x.tip}>{x.t}</span>{:else}<span class="ind">🎙 в разговоре</span>{/each}</div>
                <span class="small muted tcs">{t.cs || ' '}</span>
                {#if t.mine}
                  <div class="tact"></div>
                {:else}
                  <div class="tact">
                    <button class="b sm ib" title={sts?.silent ? 'У него полная тишина — пинг не прозвучит' : 'Пинг — тихий сигнал у него'} disabled={!!sts?.silent} onclick={() => ping(t.name)}><Gi n="bell" /></button>
                    <button class="b sm ib" title="Заглушить: отошёл, а у него шумит. Включить микрофон сможет только он сам" disabled={!(t.st && t.st.voice !== false && t.st.mic && !t.st.fm)} onclick={() => forceMute(t.name)}><Gi n="micoff" /></button>
                    <button class="b sm ib" title={'Громкость: ' + Math.round((R.vol[t.name] ?? 1) * 100) + '%'} disabled={!(t.st && t.st.voice && rm.voice)} onclick={e => popover(e.currentTarget as Element, 'vol-' + t.name, VolPop, { n: t.name })}><Gi n="sound" /></button>
                    {#if streams.length}
                      <button class="b sm ib" title={'Смотреть показ: ' + streams.map((s: any) => s.label).join(', ')} onclick={() => watchAllOf(t.name)}><Gi n="eye" /></button>
                    {:else}
                      <button class="b sm ib" title="Попросить показать экран" onclick={() => askScreen(t.name)}><Gi n="hand" /></button>
                    {/if}
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        </div>
        <div class="vview" data-vv="shows" class:hide={VV.view !== 'shows'}>
          <div class="vsub"><div class="vtabs sm"><button class:on={VV.sub === 'list'} onclick={() => (VV.sub = 'list')}>Показы</button><button class:on={VV.sub === 'cfg'} onclick={() => (VV.sub = 'cfg')}>Настройки показа</button></div><div class="grow"></div>
            <button class="b sm ib" title="Настройки показа" onclick={() => (VV.sub = VV.sub === 'cfg' ? 'list' : 'cfg')}><Gi n="gear" /></button></div>
          <div class:hide={VV.sub !== 'list'}><Streams /></div>
          <div class="vcfg" class:hide={VV.sub !== 'cfg'}>
            <div class="setrow"><span>Качество показа</span><Seg k="shareQ" opts={[['text', 'Чёткий текст · 30 к/с'], ['motion', 'Плавность · 60 к/с'], ['low', 'Экономно']]} /><span></span></div>
            <div class="setrow"><span>Звук системы</span><label class="small"><input type="checkbox" checked={SET.shareAudio} onchange={e => setSet('shareAudio', (e.target as HTMLInputElement).checked)} /> вместе с экраном</label><span></span></div>
            <div class="setrow"><span>Курсор</span><label class="small"><input type="checkbox" checked={SET.shareCursor} onchange={e => setSet('shareCursor', (e.target as HTMLInputElement).checked)} /> показывать указатель мыши</label><span></span></div>
            <div class="small muted">Применяется к следующему показу. Кнопка «Экран» внизу только начинает показ.</div>
          </div>
        </div>
      {/if}
      <div class="vview" data-vv="board" class:hide={!S.room || VV.view !== 'board'}><div id="boardHost" bind:this={boardHost}></div></div>
      {#if S.room}
        {@const rm = S.room}
        <footer class="dock">
          <button class="ctl" class:off={!rm.mic || !!rm.forced} class:forced={!!rm.forced} disabled={!rm.voice} title={rm.forced ? 'Вас заглушил ' + titleOf(rm.forced) + '. Нажмите, чтобы снова говорить' : 'Включить или выключить микрофон'} onclick={toggleMic}>
            <Gi n="mic2" /><span>{rm.forced ? 'Заглушили' : rm.voice && rm.mic ? 'Микрофон' : 'Микрофон выкл'}</span>
            <i class="chev" title="Микрофон: устройство и громкость" role="button" tabindex="-1" onclick={e => pop(e, 'mic', MicPop)} onkeydown={() => {}}><Gi n="chev" /></i></button>
          <button class="ctl" class:off={rm.deaf} disabled={!rm.voice} onclick={toggleDeaf}><Gi n="head" /><span>{rm.deaf ? 'Звук выкл' : 'Звук'}</span>
            <i class="chev" title="Звук: устройство и громкость" role="button" tabindex="-1" onclick={e => pop(e, 'out', OutPop)} onkeydown={() => {}}><Gi n="chev" /></i></button>
          <button class="ctl" class:act={rm.ptt} disabled={!rm.voice} aria-pressed={rm.ptt} onclick={() => setPtt(!rm.ptt)}><Gi n="ptt" /><span>{rm.ptt ? (rm.pttDown ? 'Говорю' : keyName(SET.pttKey)) : 'Рация'}</span></button>
          <i class="dsep"></i>
          <button class="ctl" class:act={rm.nmine > 0} onclick={shareStart}><Gi n="shareUp" /><span>Экран</span><b class="cnt red dcnt" class:hide={!rm.nmine}>{rm.nmine || ''}</b></button>
          <button class="ctl" class:act={VV.view === 'board'} disabled={!rm.chan} title={rm.chan ? 'Общая доска канала' : 'Доска есть только в канале (не в звонке)'} onclick={() => (VV.view = VV.view === 'board' ? 'people' : 'board')}><Gi n="board" /><span>Доска</span></button>
          <div class="grow"></div>
          <button class="ctl" class:act={!chatFold} onclick={chatToggle}><Gi n="chat" /><span>Чат</span><b class="cnt dcnt" class:hide={!rm.unread || !chatFold}>{rm.unread || ''}</b></button>
          <button class="ctl" onclick={e => popover(e.currentTarget as Element, 'cfg', CfgPop)}><Gi n="gear" /><span>Канал</span></button>
          <button class="ctl leave" onclick={() => roomLeave()}><Gi n="leave" /><span>Выйти</span></button>
        </footer>
      {/if}
    </div>
  </section>
  <div class="card vchat" bind:this={chat} role="complementary" aria-label="Чат канала">
    <div class="vrow"><span class="cap">Чат канала</span><div class="grow"></div><button class="b sm ib" title="Свернуть чат" onclick={() => { setSet('chatFold', true); VV.chatPinned = false; }}><Gi n="railR" /></button></div>
    {#if S.key}{#key S.key}<ChatBox id="roomChat" conv={'ch:' + S.key} placeholder="Написать в канал…" />{/key}{/if}
  </div>
</div>
