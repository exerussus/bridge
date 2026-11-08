<script lang="ts">
  // «Соседи» (3.21): сосед — одна строка (статус, через что связь, пинг), подробности раскрываются.
  // Подвкладки: соседи, сеть и сервер, этот ПК, стучались.
  import { ui, toast, load, openModal } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, copy, size } from '../lib/util';
  import Gi from '../components/Gi.svelte';
  import CheckPeer from '../modals/CheckPeer.svelte';
  import ConnectPeer from '../modals/ConnectPeer.svelte';
  import InvitePeer from '../modals/InvitePeer.svelte';
  import EditPeer from '../modals/EditPeer.svelte';
  import RelayCard from '../components/RelayCard.svelte';
  import NetCard from '../components/NetCard.svelte';
  import { removePeer } from '../lib/peers';

  // что за адрес: по нему видно, через что связь — Radmin, своя сеть, домашняя сеть
  function kindOf(hp: string): string {
    if (hp === 'сервер') return 'свой сервер';
    const o = hp.replace(/:\d+$/, '').split('.').map(Number);
    return o[0] === 26 ? 'Radmin' : o[0] === 10 && o[1] === 77 ? 'своя сеть' : o[0] === 100 && o[1] >= 64 && o[1] < 128 ? 'Tailscale'
      : (o[0] === 192 && o[1] === 168) || o[0] === 10 || (o[0] === 172 && o[1] >= 16 && o[1] < 32) ? 'локальная сеть' : o[0] === 127 ? 'этот ПК' : 'адрес';
  }
  const hostOf = (u: string) => (u || '').replace('http://', '').replace(/\/$/, '');
  const ipOf = (u: string) => hostOf(u).replace(/:\d+$/, '');

  const ov = $derived(ui.ov);
  const strangers = $derived(ov.strangers || []);
  const live = $derived(ov.peers.filter((p: any) => p.enabled && !p.invite));
  const on = $derived(live.filter((p: any) => p.online).length);
  let filter = $state('all');
  let q = $state('');
  const ok = (p: any) => p.online && !p.inboundError && !(p.remote && p.remote.error);
  const shown = $derived(ov.peers.filter((p: any) => {
    if (filter === 'on' && !p.online) return false;
    if (filter === 'off' && p.online) return false;
    const w = q.trim().toLowerCase();
    return !w || [p.name, p.title, p.url, p.alt, ...(p.ips || [])].some((x: string) => (x || '').toLowerCase().includes(w));
  }));
  $effect(() => { if (!ui.peerOpen && ov.peers.length === 1) ui.peerOpen = ov.peers[0].name; });
  const toggleOpen = (n: string) => (ui.peerOpen = ui.peerOpen === n ? '' : n);

  const HUE = ['56,225,240', '93,224,142', '255,181,71', '160,140,255', '255,120,180'];
  const hue = (n: string) => HUE[[...n].reduce((a, c) => a + c.charCodeAt(0), 0) % HUE.length];
  const stateOf = (p: any): [string, string] => !p.enabled ? ['выключен', ''] : p.invite ? ['ждём подключения', 'accf'] : p.online ? (ok(p) ? ['на связи', 'okf'] : ['связь в одну сторону', 'warnf']) : p.pending ? ['подключается', 'warnf'] : ['нет связи', 'badf'];
  const dotOf = (p: any) => (!p.enabled ? '' : p.invite || p.pending ? 'warn' : ok(p) ? 'ok' : p.online ? 'warn' : 'bad');
  const sub = (p: any) => {
    if (!p.enabled) return 'мост с ним не связывается';
    if (p.invite) return 'сосед подключится по адресу этого ПК и ключу';
    if (p.online) return (p.via ? 'через ' + kindOf(p.via) + (p.via !== 'сервер' ? ' · ' + ipOf(p.via) : '') + (p.viaMain ? '' : ' (запасной)') : 'на связи') + (p.filesVia ? ' · файлы через сервер' : '');
    return p.lastSeen ? 'последний раз ' + ago(p.lastSeen) : p.error || 'ещё не проверено';
  };
  const pendOf = (p: any) => ov.mailbox?.pending?.[p.name] || { files: 0, bytes: 0 };

  async function toggle(p: any) {
    try { await api('POST', 'peers', { name: p.name, enabled: !p.enabled }); toast(p.enabled ? 'Выключен — мост с ним не связывается' : 'Включён', 'ok'); await load(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function rename(p: any) {
    if (!confirm('Переименовать соседа «' + p.name + '» в «' + p.announcedName + '»?\n\nИмя поменяется в пространствах, правилах приёма и доступе к папкам. Копии его общих папок появятся в новой папке, старые останутся на диске.')) return;
    try { await api('POST', 'peers/rename?' + new URLSearchParams({ name: p.name, to: p.announcedName })); toast('Переименован', 'ok'); await load(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function now(p: any) {
    try { const r = await api('POST', 'sync?peer=' + encodeURIComponent(p.name)); toast(r.online ? 'Обмениваемся с ' + (p.title || p.name) : (p.title || p.name) + ': ' + (r.error || 'нет связи'), r.online ? 'ok' : 'bad'); load(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  const known = (s: any) => ov.peers.find((p: any) => (s.kid ? p.keyId === s.kid : p.name.toLowerCase() === s.name.toLowerCase()));
  const t = (m: string, k?: string) => toast(m, k);
  const SUBS: [string, string][] = [['list', 'Соседи'], ['net', 'Сеть и сервер'], ['pc', 'Этот ПК'], ['str', 'Стучались']];
  const chev = '<path d="M6 9l6 6 6-6"/>';
</script>

<div class="row" style="gap:10px;flex-wrap:wrap;margin:-6px 0 16px">
  <div class="seg2" role="tablist" aria-label="Разделы">
    {#each SUBS as [k, l]}
      <button role="tab" class:on={ui.peersSub === k} onclick={() => (ui.peersSub = k)}>{l}
        {#if k === 'list'}<span class="cnt2">{ov.peers.length}</span>{:else if k === 'str' && strangers.length}<span class="cnt2 amb">{strangers.length}</span>{/if}</button>
    {/each}
  </div>
  <div class="grow"></div>
  {#if ui.peersSub === 'list'}
    <span class="small muted">{#if live.length}<b style="color:var(--ok)">{on}</b> на связи из {live.length}{/if}</span>
    <button class="b" title="Пошагово проверить связь и обмен со всеми соседями" onclick={() => openModal(CheckPeer, { name: null })}><Gi n="pulse" /> Проверить всех</button>
    <button class="b" title="Создать ключ: сосед подключится сам по адресу этого ПК и ключу" onclick={() => openModal(InvitePeer)}><Gi n="mail" /> Пригласить</button>
    <button class="b pri" title="Подключить по адресу соседа и ключу — имя он сообщит сам" onclick={() => openModal(ConnectPeer, {})}>＋ Подключить</button>
  {/if}
</div>

{#if ui.peersSub === 'list'}
  {#if ov.peers.length > 3}
    <div class="row" style="gap:10px;margin-bottom:12px;flex-wrap:wrap">
      <div class="seg2"><button class:on={filter === 'all'} onclick={() => (filter = 'all')}>Все</button><button class:on={filter === 'on'} onclick={() => (filter = 'on')}>На связи · {ov.peers.filter((p: any) => p.online).length}</button><button class:on={filter === 'off'} onclick={() => (filter = 'off')}>Нет связи · {ov.peers.filter((p: any) => !p.online).length}</button></div>
      <input bind:value={q} placeholder="Поиск по имени или адресу" aria-label="Поиск соседа" style="width:240px" />
    </div>
  {/if}
  <div class="plist">
    {#each shown as p (p.name)}
      {@const open = ui.peerOpen === p.name}
      {@const [st, sc] = stateOf(p)}
      <div class="card prow" class:open>
        <div class="phead">
          <button class="pbtn" onclick={() => toggleOpen(p.name)} aria-expanded={open}>
            <span class="av2" style="background:rgba({hue(p.name)},.12);color:rgb({hue(p.name)})">{(p.title || p.name).slice(0, 1).toUpperCase()}<span class="dot {dotOf(p)}"></span></span>
            <span class="pname"><b class="ell">{p.title || p.name}</b><span class="mono small muted ell">{p.name}{p.pending ? ' (временное)' : ''}</span></span>
            <span class="pstate"><span class="pill plain {sc}">{st}</span><span class="small muted ell" title={sub(p)}>{sub(p)}</span></span>
            <span class="mono small muted pms">{p.online && p.latency != null ? p.latency + ' мс' : '—'}</span>
          </button>
          <button class="b sm" title="пошагово: связь, ключ, адрес, часы, пространства, папки" onclick={() => openModal(CheckPeer, { name: p.name })}><Gi n="pulse" /> Проверить</button>
          <button class="b sm sq" aria-label={open ? 'Свернуть' : 'Подробнее'} onclick={() => toggleOpen(p.name)}><svg class="chev" class:up={open} viewBox="0 0 24 24" aria-hidden="true">{@html chev}</svg></button>
        </div>
        {#if open}
          <div class="pbody">
            {#if p.enabled && !p.invite && !p.online}
              <div class="pban bad"><span class="dot bad"></span><div class="grow"><b>{p.error || 'Ещё не проверено'}</b>{#if p.hint}<div class="small muted">{p.hint}</div>{/if}</div><button class="b pri sm" onclick={() => openModal(CheckPeer, { name: p.name })}>Проверить по шагам</button></div>
            {/if}
            {#if p.inboundError}<div class="pban bad"><span class="dot bad"></span><div class="grow"><b>Здесь не пускаем его запросы</b><div class="small muted">{p.inboundError}</div></div></div>{/if}
            {#if p.remote && p.remote.error}<div class="pban bad"><span class="dot bad"></span><div class="grow"><b>Он не может забирать отсюда</b><div class="small muted">у него: {p.remote.error}</div></div></div>{/if}
            {#if p.warning}<div class="pban warn"><span class="dot warn"></span><div class="grow">{p.warning}</div>{#if p.announcedName}<button class="b sm" onclick={() => rename(p)}>Принять имя «{p.announcedName}»</button>{/if}</div>{/if}
            {#if p.invite}<div class="pban"><div class="grow small muted">Приглашение: сосед подключится по адресу этого ПК и ключу. Строку приглашения можно создать заново — «Пригласить».</div></div>{/if}
            <div class="pcols">
              <div class="pcol"><h3>Связь</h3>
                <div class="kv2"><span>→ мы к нему</span><span class="v"><span class="dot {p.online ? 'ok' : p.enabled ? 'bad' : ''}"></span><span class="vt">{p.online ? 'на связи · ' + ago(p.lastSeen) : p.enabled ? 'нет ответа' : 'выключен'}</span></span></div>
                <div class="kv2"><span>← он к нам</span><span class="v"><span class="dot {p.inboundError || (p.remote && p.remote.error) ? 'bad' : p.lastInbound ? 'ok' : ''}"></span><span class="vt">{p.inboundError ? 'отказ' : p.remote && p.remote.error ? 'не забирает' : p.lastInbound ? 'забирает · ' + ago(p.lastInbound) : 'ещё не обращался'}</span></span></div>
                <div class="kv2"><span>Версия</span><span class="v"><span class="vt">{p.version ? 'v' + p.version : '—'}{p.legacy ? ' — старая, обновите мост там' : ''}</span></span></div>
                {#if p.status && p.status.waitingApproval}<div class="kv2"><span>Ждёт у него</span><span class="v"><span class="vt">{p.status.waitingApproval} подтвержд.</span></span></div>{/if}
              </div>
              <div class="pcol"><h3>Адреса</h3>
                <div class="kv2"><span>{kindOf(hostOf(p.url))}</span><span class="v"><span class="dot {p.online && p.viaMain ? 'ok' : ''}"></span><span class="vt mono" title={'основной: ' + hostOf(p.url)}>{ipOf(p.url) || '—'}{p.online && p.viaMain ? ' · сейчас' : ''}</span></span></div>
                {#if p.alt}<div class="kv2"><span>{kindOf(p.alt)}</span><span class="v"><span class="dot {p.online && !p.viaMain && p.via === p.alt ? 'ok' : ''}"></span><span class="vt mono" title={'запасной: ' + p.alt}>{ipOf(p.alt)} · запасной{p.online && !p.viaMain && p.via === p.alt ? ', сейчас' : ''}</span></span></div>{/if}
                {#if p.online && p.via === 'сервер'}<div class="kv2"><span>Свой сервер</span><span class="v"><span class="dot ok"></span><span class="vt">сейчас через сервер</span></span></div>{/if}
                {#if p.ips.length}<div class="kv2"><span>Принимаем с</span><span class="v"><span class="vt mono" title="принимаем запросы с этих адресов; не использованные больше 3 дней убираются сами (кроме основного, запасного и своей сети)">{#each p.ips as ip, i}<span title={p.ipSeen && p.ipSeen[ip] ? 'последний раз: ' + p.ipSeen[ip] : 'ещё не использовался'}>{ip}</span>{i < p.ips.length - 1 ? ', ' : ''}{/each}</span></span></div>{/if}
              </div>
              <div class="pcol"><h3>Обмен</h3>
                <div class="kv2"><span>Файлы</span><span class="v"><span class="vt">{p.filesVia ? 'через свой сервер' : 'напрямую'}</span></span></div>
                <div class="kv2"><span>Ждут отправки</span><span class="v"><span class="vt">{pendOf(p).files ? pendOf(p).files + ' файл. · ' + size(pendOf(p).bytes) : 'ничего'}</span></span></div>
                <div class="kv2"><span>Принято всего</span><span class="v"><span class="vt">{p.filesIn ? p.filesIn + ' файл.' : '—'}</span></span></div>
                {#each p.issues || [] as i}<div class="kv2"><span>Пространство</span><span class="v"><span class="dot warn"></span><span class="vt" title={i}>{i}</span></span></div>{/each}
              </div>
            </div>
            <div class="row pfoot">
              <h3 style="margin:0">Пространства</h3>
              {#each p.shares as s}<span class="chip">{s}</span>{:else}<span class="small muted">нет</span>{/each}
              <div class="grow"></div>
              {#if p.enabled && p.online}<button class="b sm" title="Обменяться с ним сейчас: забрать новое и попросить забрать наше" onclick={() => now(p)}><Gi n="sync" /> Обменяться</button>{/if}
              <button class="b sm" onclick={() => openModal(EditPeer, { name: p.name })}>Изменить</button>
              <button class="b sm" title={p.enabled ? 'временно не связываться (настройки сохранятся)' : 'снова связываться'} onclick={() => toggle(p)}>{p.enabled ? 'Пауза' : 'Включить'}</button>
              <button class="b sm" data-open="outgoing" data-peer={p.name}><Gi n="folder" /> Папка отправки</button>
              <button class="b sm danger" onclick={() => removePeer(p)}>Удалить</button>
            </div>
          </div>
        {/if}
      </div>
    {:else}
      <div class="card"><div class="empty">{ov.peers.length ? 'Никого не нашлось' : 'Соседей нет — «Пригласить» здесь или «＋ Подключить» по строке с того ПК'}</div></div>
    {/each}
  </div>
{:else if ui.peersSub === 'net'}
  <RelayCard />
  <NetCard />
{:else if ui.peersSub === 'pc'}
  <div class="grid">
    <div class="card"><h3>Этот ПК</h3>
      <div class="kv">
        <div>Узел</div><div><b>{ov.node.node}</b> <button class="b sm" onclick={() => copy(ov.node.node, t)}>копировать</button></div>
        <div>Адреса ПК</div><div>{#each ov.localIps as ip}<button class="tag" title="копировать" onclick={() => copy(ip, t)}>{ip}</button> {/each}</div>
        <div>Для соседей</div><div>{#each (ov.announce || []).map((u: string) => u.replace('http://', '')) as a}<button class="tag" title="копировать" onclick={() => copy(a, t)}>{a}</button> {/each}</div>
        <div>Слушает</div><div class="mono">{#each ov.listen as l}{l}<br />{/each}</div>
        <div>Настройки</div><div class="mono">{ov.configFile || '—'}</div>
        <div>Папка узла</div><div><button class="b sm" data-open="node"><Gi n="folder" /> Открыть</button></div>
      </div>
      <div class="small muted" style="margin-top:8px">Изменить «listen» (свои адреса) — в bridge.json, затем перезапустить узел.</div>
    </div>
    <div class="card"><h3>Как подключить нового соседа</h3>
      <ol class="small" style="margin:0;padding-left:18px">
        <li>Здесь — «Пригласить». Появится строка: <b>адрес этого ПК и ключ</b>.</li>
        <li>Передайте строку на тот ПК (лично или в надёжном мессенджере — ключ как пароль).</li>
        <li>Там — «＋ Подключить» и вставить строку. Всё: имена ПК и адреса мосты узнают друг у друга сами.</li>
        <li>Брандмауэр на обоих ПК: разрешить «Мост» для нужной сети.</li>
      </ol>
      <div class="small muted" style="margin-top:6px">Не подключается — «Проверить» у соседа: проверка по шагам скажет, где обрыв и что сделать.</div>
    </div>
  </div>
{:else}
  {#if strangers.length}
    <div class="small muted" style="margin-bottom:10px">Эти узлы пытались связаться с этим ПК, но не были пущены: ключ не подходит, адрес не записан или сосед выключен. Если это ваш ПК — подключите его отсюда (адрес подставится) с тем же ключом, что у него, или добавьте адрес.</div>
    <div class="plist">
      {#each strangers as s (s.name + s.ip)}
        {@const k = known(s)}
        <div class="card pban warn" style="padding:14px 18px">
          <span class="av2" style="background:var(--warn-soft);color:var(--warn)">?</span>
          <div class="grow"><b>{s.name}</b> <span class="mono small muted">· {s.ip}</span>{#if k}<span class="small muted"> · есть в соседях: {k.title || k.name}</span>{/if}<div class="small muted">{s.why} · {ago(s.time)}</div></div>
          {#if k}<button class="b sm pri" onclick={() => openModal(EditPeer, { name: k.name, extraIp: s.ip })}>{k.enabled ? 'Добавить адрес' : 'Изменить'}</button>
          {:else}<button class="b sm pri" onclick={() => openModal(ConnectPeer, { ip: s.ip, title: s.name })}>＋ Подключить</button>{/if}
        </div>
      {/each}
    </div>
  {:else}<div class="card"><div class="empty">Никто не стучался.</div></div>{/if}
{/if}

<style>
  .plist { display: flex; flex-direction: column; gap: 10px; }
  .prow { padding: 0; overflow: hidden; }
  .prow.open { border-color: rgba(56, 225, 240, 0.28); }
  .phead { display: flex; align-items: center; gap: 10px; padding: 12px 14px 12px 18px; }
  .pbtn { flex: 1; min-width: 0; display: flex; align-items: center; gap: 14px; border: 0; background: none; color: inherit; font: inherit; text-align: left; padding: 0; cursor: pointer; }
  .pname { flex: 0 1 210px; min-width: 0; display: flex; flex-direction: column; }
  .pname b { font-size: 15px; }
  .pstate { flex: 1 1 260px; min-width: 0; display: flex; gap: 10px; align-items: center; }
  .pms { width: 62px; text-align: right; flex: none; }
  .pbody { border-top: 1px solid var(--line); padding: 16px 18px; display: flex; flex-direction: column; gap: 14px; }
  .pban { display: flex; gap: 14px; align-items: center; padding: 12px 14px; border-radius: 12px; background: var(--panel2); border: 1px solid var(--line); flex-wrap: wrap; }
  .pban.bad { background: var(--bad-soft); border-color: rgba(255, 79, 100, 0.22); }
  .pban.warn { background: var(--warn-soft); border-color: rgba(255, 181, 71, 0.25); }
  .pcols { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 14px; }
  .pcol { background: var(--panel2); border-radius: 12px; padding: 12px 14px; min-width: 0; }
  .pcol h3 { margin: 0 0 6px; }
  .pfoot { gap: 8px; flex-wrap: wrap; }
  .tag { cursor: pointer; color: var(--text); }
  @media (max-width: 700px) { .pstate .ell { display: none; } }
</style>
