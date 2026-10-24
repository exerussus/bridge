<script lang="ts">
  import { ui, toast, load, openModal } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, copy } from '../lib/util';
  import Gi from '../components/Gi.svelte';
  import CheckPeer from '../modals/CheckPeer.svelte';
  import ConnectPeer from '../modals/ConnectPeer.svelte';
  import InvitePeer from '../modals/InvitePeer.svelte';
  import EditPeer from '../modals/EditPeer.svelte';
  import { removePeer } from '../lib/peers';

  const ov = $derived(ui.ov);
  const strangers = $derived(ov.strangers || []);

  async function toggle(p: any) {
    try { await api('POST', 'peers', { name: p.name, enabled: !p.enabled }); toast(p.enabled ? 'Выключен — мост с ним не связывается' : 'Включён', 'ok'); await load(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function rename(p: any) {
    if (!confirm('Переименовать соседа «' + p.name + '» в «' + p.announcedName + '»?\n\nИмя поменяется в пространствах, правилах приёма и доступе к папкам. Копии его общих папок появятся в новой папке, старые останутся на диске.')) return;
    try { await api('POST', 'peers/rename?' + new URLSearchParams({ name: p.name, to: p.announcedName })); toast('Переименован', 'ok'); await load(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  const known = (s: any) => ov.peers.find((p: any) => (s.kid ? p.keyId === s.kid : p.name.toLowerCase() === s.name.toLowerCase()));
  const dotOf = (p: any) => (p.online && !p.inboundError && !(p.remote && p.remote.error) ? 'ok' : p.enabled ? (p.invite || p.pending ? 'warn' : 'bad') : '');
  const t = (m: string, k?: string) => toast(m, k);
</script>

<div class="card">
  <div class="row" style="margin-bottom:8px">
    <h3 style="margin:0">Соседи</h3><div class="grow"></div>
    <button class="b" title="Пошагово проверить связь и обмен со всеми соседями" onclick={() => openModal(CheckPeer, { name: null })}><Gi n="pulse" /> Проверить всех</button>
    <button class="b" title="Создать ключ: сосед подключится сам по адресу этого ПК и ключу" onclick={() => openModal(InvitePeer)}><Gi n="mail" /> Пригласить</button>
    <button class="b pri" title="Подключить по адресу соседа и ключу — имя он сообщит сам" onclick={() => openModal(ConnectPeer, {})}>＋ Подключить</button>
  </div>
  <table>
    <thead><tr><th>Псевдоним / узел</th><th>Адрес</th><th>Связь</th><th>Пространства</th><th></th></tr></thead>
    <tbody>
      {#each ov.peers as p (p.name)}
        <tr>
          <td><span class="dot {dotOf(p)}"></span> <b>{p.title || p.name}</b><div class="small muted mono">{p.name}{p.pending ? ' (временное)' : ''}</div></td>
          <td class="mono small">{(p.url || '—').replace('http://', '')}{#if p.ips.length}<div class="muted" title="принимаем запросы с этих адресов">{p.ips.join(', ')}</div>{/if}</td>
          <td class="small">
            {#if !p.enabled}<span class="pill">выключен</span>
            {:else if p.invite}<span class="pill acc">ждём подключения</span><div class="muted">приглашение: сосед подключится по адресу этого ПК и ключу</div>
            {:else}
              {#if p.online}
                <div><span class="pill ok">→ на связи</span> <span class="muted">{ago(p.lastSeen)}{p.latency != null ? ' · ' + p.latency + ' мс' : ''}{p.version ? ' · v' + p.version : ''}{p.status && p.status.waitingApproval ? ' · ждёт подтверждения: ' + p.status.waitingApproval : ''}</span></div>
              {:else}
                <div><span class="pill bad">→ нет связи</span> {p.error || 'ещё не проверено'}</div>{#if p.hint}<div class="muted">{p.hint}</div>{/if}
              {/if}
              {#if p.inboundError}<div style="margin-top:3px"><span class="pill bad">← отказ</span> здесь не пускаем: {p.inboundError}</div>
              {:else if p.remote && p.remote.error}<div style="margin-top:3px"><span class="pill bad">← не забирает</span> у него: {p.remote.error}</div>
              {:else if p.lastInbound}<div class="muted" style="margin-top:3px">← забирает отсюда · {ago(p.lastInbound)}</div>
              {:else if p.online}<div class="muted" style="margin-top:3px">← сюда ещё не обращался</div>{/if}
              {#if p.pending}<div class="muted">имя узнается, когда сосед ответит</div>{/if}
              {#if p.warning}<div style="margin-top:3px"><span class="pill warn">имя</span> {p.warning}{#if p.announcedName} <button class="b sm" onclick={() => rename(p)}>Принять имя «{p.announcedName}»</button>{/if}</div>{/if}
              {#if p.legacy}<div class="muted">сосед на старой версии — обновите мост там до текущей</div>{/if}
              {#each p.issues || [] as i}<div style="margin-top:3px"><span class="pill warn">пространство</span> {i}</div>{/each}
            {/if}
          </td>
          <td>{#each p.shares as s}<span class="tag">{s}</span> {/each}</td>
          <td style="white-space:nowrap">
            <button class="b sm pri" title="пошагово: связь, ключ, адрес, часы, пространства, папки" onclick={() => openModal(CheckPeer, { name: p.name })}><Gi n="pulse" /> Проверить</button>
            <button class="b sm" onclick={() => openModal(EditPeer, { name: p.name })}>Изменить</button>
            <button class="b sm" title={p.enabled ? 'временно не связываться (настройки сохранятся)' : 'снова связываться'} onclick={() => toggle(p)}><Gi n={p.enabled ? 'pause' : 'play'} /></button>
            <button class="b sm" data-open="outgoing" data-peer={p.name} title="папка отправки"><Gi n="folder" /></button>
            <button class="b sm danger" title="удалить соседа" onclick={() => removePeer(p)}><Gi n="trash" /></button>
          </td>
        </tr>
      {:else}
        <tr><td colspan="5" class="empty">Соседей нет — «✉ Пригласить» здесь или «＋ Подключить» по строке с того ПК</td></tr>
      {/each}
    </tbody>
  </table>
</div>

<div class="card" class:hide={!strangers.length} style="margin-top:14px;border-color:var(--warn)">
  <h3>⚠ Стучались, но не узнаны</h3>
  <div class="small muted" style="margin-bottom:6px">Эти узлы пытались связаться с этим ПК, но не были пущены: ключ не подходит, адрес не записан или сосед выключен. Если это ваш ПК — подключите его отсюда (адрес подставится) с тем же ключом, что у него, или добавьте адрес.</div>
  <table>
    <thead><tr><th>Назвался</th><th>Адрес</th><th>Почему отказ</th><th>Когда</th><th></th></tr></thead>
    <tbody>
      {#each strangers as s (s.name + s.ip)}
        {@const k = known(s)}
        <tr>
          <td><b class="mono">{s.name}</b>{#if k}<div class="small muted">есть в соседях: {k.title || k.name}</div>{/if}</td>
          <td class="mono">{s.ip}</td><td class="small">{s.why}</td><td class="small muted">{ago(s.time)}</td>
          <td>
            {#if k}<button class="b sm pri" onclick={() => openModal(EditPeer, { name: k.name, extraIp: s.ip })}>{k.enabled ? 'Добавить адрес' : 'Изменить'}</button>
            {:else}<button class="b sm pri" onclick={() => openModal(ConnectPeer, { ip: s.ip, title: s.name })}>＋ Подключить</button>{/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<div class="grid" style="margin-top:14px">
  <div class="card"><h3>Этот ПК</h3>
    <div class="kv">
      <div>Узел</div><div><b>{ov.node.node}</b> <button class="b sm" onclick={() => copy(ov.node.node, t)}>копировать</button></div>
      <div>Адреса ПК</div><div>{#each ov.localIps as ip}<span class="tag" style="cursor:pointer" title="копировать" onclick={() => copy(ip, t)} role="button" tabindex="0" onkeydown={() => {}}>{ip}</span> {/each}</div>
      <div>Для соседей</div><div>{#each (ov.announce || []).map((u: string) => u.replace('http://', '')) as a}<span class="tag" style="cursor:pointer" title="копировать" onclick={() => copy(a, t)} role="button" tabindex="0" onkeydown={() => {}}>{a}</span> {/each}</div>
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
