<script lang="ts">
  // Своя локальная сеть MostNet (3.17): служба most-net на этом ПК, адрес 10.77.x.y, соседи по сети.
  // Ничего не прыгает: строки состояния — постоянной высоты, кнопки не прячутся и не меняют ширину.
  import { onMount, onDestroy } from 'svelte';
  import { api } from '../lib/api';
  import { toast } from '../lib/state.svelte';
  import { copy } from '../lib/util';

  let d: any = $state({});
  let busy = $state('');
  let timer: any;
  const svc = $derived(d.service || null);

  async function load() { try { d = await api('GET', 'mostnet'); } catch {} }
  async function act(kind: string, path: string, body?: any, ok?: string) {
    busy = kind;
    try { const r = await api('POST', path, body); if (r.warning) toast(r.warning, 'bad'); else if (ok) toast(ok, 'ok'); }
    catch (e: any) { toast(e.message, 'bad'); }
    finally { busy = ''; await load(); }
  }
  const install = () => act('install', 'mostnet/install', undefined, 'Служба MostNet поставлена');
  const remove = () => { if (confirm('Убрать службу своей сети с этого ПК?\n\nАдаптер MostNet исчезнет; ключ сети и адрес сохранятся — поставить снова можно в любой момент.')) act('remove', 'mostnet/uninstall', undefined, 'Служба убрана'); };
  const toggle = () => act('toggle', 'mostnet', { enabled: !d.enabled }, d.enabled ? 'Своя сеть выключена' : 'Своя сеть включается…');

  // служба ставится копией в ProgramData и сама с Мостом не обновляется
  const outdated = $derived(!!(svc && svc.version && d.version && d.windows && svc.version !== d.version));
  const line = $derived.by(() => {
    const l = base();
    return outdated ? { dot: 'warn', text: 'служба устарела (' + svc.version + ', Мост — ' + d.version + ') — «Обновить службу» · ' + l.text } : l;
  });
  const base = () => {
    if (d.windows === false && !svc) return { dot: '', text: 'служба своей сети есть только для Windows' };
    if (!d.relay) return { dot: '', text: 'сначала подключите свой сервер-ретранслятор (выше) — сеть идёт через него' };
    if (d.relayOff) return { dot: '', text: 'свой сервер в режиме «Не пользоваться» — сеть тоже выключена' };
    if (!svc) return { dot: d.enabled ? 'warn' : '', text: d.exe ? 'служба не установлена (или не запущена) — «Установить службу», Windows спросит подтверждение администратора' : 'рядом с Most.exe нет most-net.exe и wintun.dll — положите их из publish (их собирает build.cmd)' };
    if (!d.enabled || svc.state === 'off') return { dot: '', text: 'выключена' + (svc.vip ? ' · адрес этого ПК в сети — ' + svc.vip : '') };
    if (svc.state === 'on') return { dot: 'ok', text: 'включена · этот ПК — ' + svc.vip + ' · путь до сервера ' + (svc.path === 'udp' ? 'UDP' : 'TCP 443') + (svc.rtt != null ? ' · ответ ' + svc.rtt + ' мс' : '') };
    if (svc.state === 'connecting') return { dot: 'warn', text: 'подключаюсь к серверу…' };
    return { dot: 'bad', text: svc.error || 'ошибка' };
  };
  const ago = (s: number | null) => (s == null ? '—' : s < 5 ? 'только что' : s < 60 ? s + ' с назад' : Math.floor(s / 60) + ' мин назад');
  const kb = (b: number) => (b > 1 << 20 ? (b / (1 << 20)).toFixed(1) + ' МБ' : b >= 1024 ? (b / 1024).toFixed(0) + ' КБ' : b + ' Б');

  onMount(() => { load(); timer = setInterval(load, 5000); });
  onDestroy(() => clearInterval(timer));
</script>

<div class="card" style="margin-top:14px">
  <div class="row" style="margin-bottom:6px">
    <h3 style="margin:0">Своя локальная сеть</h3><div class="grow"></div>
    <span class="small muted">как Radmin, только через ваш сервер — для игр и программ по локальной сети</span>
  </div>
  <div class="small net-line" title={line.text}><span class="dot {line.dot}"></span> {line.text}</div>

  <div class="row" style="margin-top:8px;gap:6px">
    <button class="b" style="width:190px" disabled={!!busy || !d.windows || !d.exe} title="Windows спросит подтверждение администратора — служба нужна, чтобы создать сетевой адаптер" onclick={install}>{busy === 'install' ? 'Ставлю…' : outdated ? 'Обновить службу' : svc ? 'Переустановить службу' : 'Установить службу'}</button>
    <button class="b pri" style="width:130px" disabled={!!busy || !d.relay || !svc} onclick={toggle}>{d.enabled ? 'Выключить' : 'Включить'}</button>
    <button class="b sm" disabled={!!busy || !svc} title="копировать адрес этого ПК" onclick={() => svc && svc.vip && copy(svc.vip, (m: string, k?: string) => toast(m, k))}>Копировать адрес</button>
    <div class="grow"></div>
    <button class="b danger" disabled={!!busy || !svc} title="убрать службу с этого ПК" onclick={remove}>Убрать службу</button>
  </div>

  <table style="margin-top:10px">
    <thead><tr><th>Сосед по сети</th><th>Адрес</th><th>Последний пакет</th><th>Трафик ↑/↓</th></tr></thead>
    <tbody>
      {#each (svc && svc.peers) || [] as p (p.vip)}
        <tr><td><b>{p.name}</b></td><td class="mono">{p.vip}</td><td class="small muted">{ago(p.lastRx)}</td><td class="small">{kb(p.tx)} / {kb(p.rx)}</td></tr>
      {:else}
        <tr><td colspan="4" class="empty small">{svc && svc.state === 'on' ? 'других ПК в сети пока нет — другу нужен свой доступ к серверу (приглашение most-relay invite) и включённая сеть' : 'соседи появятся, когда сеть включена'}</td></tr>
      {/each}
    </tbody>
  </table>
  <div class="small muted" style="margin-top:6px">Игры и программы видят друг друга по адресам 10.77.x.y; поиск серверов в локальной сети (широковещанием) тоже работает. Пакеты шифруются от ПК до ПК — сервер их не читает.</div>
  <details class="small" style="margin-top:6px"><summary class="muted">Журнал службы</summary><pre class="mono small" style="white-space:pre-wrap;max-height:200px;overflow:auto;margin:6px 0 0">{(d.log || []).join('\n') || d.installLog || '—'}</pre></details>
</div>

<style>
  .net-line { height: 20px; line-height: 20px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
