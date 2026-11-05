<script lang="ts">
  // Свой сервер-ретранслятор (most-relay, 3.16): строка приглашения, режим, «делиться с соседями», состояние.
  // Ничего не прыгает: строки состояния — фиксированной высоты, кнопки не прячутся (только серые).
  import { onMount, onDestroy } from 'svelte';
  import { api } from '../lib/api';
  import { toast } from '../lib/state.svelte';

  let st: any = $state({});
  let fromPeers: any[] = $state([]);
  let files = $state(false), net = $state(false), netUp = $state(false);
  let line = $state('');
  let busy = $state(false);
  let timer: any;

  async function load() {
    try { const r = await api('GET', 'relay'); st = r.status || {}; fromPeers = r.fromPeers || []; files = !!r.files; net = !!r.net; netUp = !!r.netUp; } catch {}
  }
  async function set(body: any, ok: string) {
    busy = true;
    try {
      const r = await api('POST', 'relay', body);
      if (r.status) st = r.status;
      await load();
      if (r.status && r.status.configured && !r.status.ok) toast(r.status.error || 'сервер не ответил', 'bad'); else toast(ok, 'ok');
      return true;
    } catch (e: any) { toast(e.message, 'bad'); return false; } finally { busy = false; }
  }
  async function save() { if (await set({ invite: line }, 'Сервер подключён')) line = ''; }
  async function check() {
    busy = true;
    try { const r = await api('POST', 'relay/check'); st = r.status || {}; toast(st.ok ? 'Сервер на связи' : st.error || 'нет связи', st.ok ? 'ok' : 'bad'); } catch (e: any) { toast(e.message, 'bad'); } finally { busy = false; }
  }
  async function remove() {
    if (!confirm('Отключить свой сервер-ретранслятор на этом ПК?\n\nГолос пойдёт как раньше — напрямую и через Radmin. Ключ этого ПК на сервере забудется — чтобы вернуть, понадобится новое приглашение (most-relay invite).')) return;
    await set({ invite: '' }, 'Сервер отключён — связь как раньше');
  }
  const filesNote = $derived(
    !files ? 'выключено — как раньше (напрямую или через Radmin); если включено у соседа — с ним через сервер'
    : !st.configured || !st.ok ? 'не действует: свой сервер не на связи'
    : st.filesAllowed === false ? 'не действует: сервер запрещает (приложение «Сервер» → «Настройки»)'
    : !net ? 'не действует: включите «Своя локальная сеть» ниже — файлы идут по ней'
    : !netUp ? 'не действует: своя сеть на этом ПК ещё не подключилась'
    : 'действует — с соседями, у которых тоже включена своя сеть' + (st.netMbit ? ' · скорость на сервере ограничена ' + st.netMbit + ' Мбит/с' : ''));
  const until = (t: number) => (t ? new Date(t * 1000).toLocaleString('ru-RU', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }) : '');
  const MODES: [string, string, string][] = [
    ['auto', 'Если напрямую не вышло', 'WebRTC сам пробует прямой путь (и Radmin) первым; сервер — только когда не прошло'],
    ['always', 'Всегда через сервер', 'голос и экран — только через сервер (собеседники не видят адрес этого ПК)'],
    ['off', 'Не пользоваться', 'ни своим сервером, ни серверами соседей'],
  ];

  onMount(() => { load(); timer = setInterval(load, 10000); });
  onDestroy(() => clearInterval(timer));
</script>

<div class="card" style="margin-top:14px">
  <div class="row" style="margin-bottom:6px">
    <h3 style="margin:0">Свой сервер-ретранслятор</h3><div class="grow"></div>
    <span class="small muted">опция: без него всё работает как раньше — напрямую и через Radmin</span>
  </div>

  <div class="small relay-line" title={st.error || ''}>
    {#if !st.configured}
      <span class="dot"></span> не подключён
    {:else if st.ok}
      <span class="dot ok"></span> на связи: <b>{st.name || st.host}</b> · {st.address} · ответ {st.rtt ?? '—'} мс · этот ПК на сервере — «{st.member}» · доступ до {until(st.expires)}{st.error ? ' · последняя проверка: ' + st.error : ''}
    {:else}
      <span class="dot bad"></span> {st.address || ''} — {st.error || 'ещё не проверено'}
    {/if}
  </div>
  <div class="small muted relay-line">
    {#if fromPeers.length}серверы соседей: {fromPeers.map((p) => (p.name || p.host) + ' (от ' + p.peer + ')').join(', ')}{:else}серверов соседей нет{/if}
  </div>

  <div class="row" style="margin-top:8px;gap:6px">
    <input class="mono grow" style="min-width:0" placeholder={st.configured ? 'другой сервер: новая строка приглашения' : 'most-relay 203.0.113.5:443 MRI-… — строка с сервера (most-relay invite, живёт 5 минут)'} bind:value={line} onkeydown={(e) => e.key === 'Enter' && line.trim() && save()} />
    <button class="b pri" style="width:132px" disabled={busy || !line.trim()} onclick={save}>{st.configured ? 'Заменить' : 'Подключить'}</button>
    <button class="b" disabled={busy || !st.configured} onclick={check}>Проверить</button>
    <button class="b danger" disabled={busy || !st.configured} title="отключить сервер на этом ПК" onclick={remove}>Убрать</button>
  </div>

  <div class="relay-modes" style="margin-top:8px">
    {#each MODES as [v, t, hint]}
      <button class="b sm" class:pri={(st.mode || 'auto') === v} title={hint} disabled={busy} onclick={() => set({ mode: v }, t)}>{t}</button>
    {/each}
  </div>
  <label class="small" style="display:block;margin-top:8px">
    <input type="checkbox" checked={st.share !== false} disabled={busy} onchange={(e) => set({ share: (e.target as HTMLInputElement).checked }, 'Сохранено')} />
    Делиться с соседями — у них сервер заработает без своей настройки (им уходит только временный ключ на сутки)
  </label>
  <label class="small" style="display:block;margin-top:6px">
    <input type="checkbox" checked={files} disabled={busy} onchange={(e) => set({ files: (e.target as HTMLInputElement).checked }, 'Сохранено')} />
    Файлы и синхронизация с соседями — через сервер (когда напрямую или через Radmin медленно)
  </label>
  <div class="small muted relay-line" style="padding-left:22px" title={filesNote}>{filesNote}</div>
  <div class="small muted" style="margin-top:6px">Как поставить сервер на VPS — <span class="mono">relay/README.md</span> в папке проекта. Приглашение печатает на сервере <span class="mono">most-relay invite</span> — оно живёт 5 минут и срабатывает один раз; дальше мост пользуется своим ключом.</div>
</div>

<style>
  .relay-line { height: 20px; line-height: 20px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .relay-modes { display: grid; grid-template-columns: repeat(3, 1fr); gap: 6px; }
</style>
