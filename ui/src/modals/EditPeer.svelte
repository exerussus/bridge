<script lang="ts">
  import { closeModal, load, toast, ui } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { copy } from '../lib/util';
  import { removePeer } from '../lib/peers';
  import Gi from '../components/Gi.svelte';
  let { name, extraIp = '' }: { name: string; extraIp?: string } = $props();
  const p = ui.ov.peers.find((x: any) => x.name === name);
  const ip0 = (p.url.match(/\/\/([^:/]+)/) || [])[1] || '';
  const port0 = (p.url.match(/:(\d+)/g) || [':5390']).pop().slice(1);
  const extraList = p.ips.filter((i: string) => i !== ip0);
  if (extraIp && extraIp !== ip0 && !extraList.includes(extraIp)) extraList.push(extraIp);
  let title = $state(p.title || '');
  let ip = $state(ip0);
  let port = $state(port0);
  let extra = $state(extraList.join(', '));
  // свой адрес соседа в своей сети (10.77.x.y), если он его уже сообщил, — подсказка для запасного
  const netIp = (p.ips || []).find((i: string) => /^10\.77\./.test(i)) || '';
  let alt = $state(p.alt || '');
  let secret = $state('');
  // (3.21.1) запасной ↔ основной: часто подключаться надо как раз по запасному (Radmin, своя сеть)
  function swapAlt() {
    const a = alt.trim().replace(/^http:\/\//, '');
    if (!a) return;
    const m = a.match(/^(.+?)(?::(\d+))?$/)!;
    const was = ip.trim() ? ip.trim() + (port ? ':' + port : '') : '';
    // (3.22.1) прежний основной адрес — и в «ещё адреса»: с него сосед по-прежнему может стучаться
    const old = ip.trim(), ex = extra.split(',').map((x: string) => x.trim()).filter(Boolean);
    if (old && old !== m[1] && !ex.includes(old)) extra = [...ex, old].join(', ');
    ip = m[1]; if (m[2]) port = m[2];
    alt = was;
    toast('Запасной стал основным — «Сохранить»');
  }
  let shares = $state<string[]>([...p.shares]);
  let enabled = $state(p.enabled);
  let made = $state('');
  async function save() {
    const body: any = { name: p.name, title, ip: ip.trim() || null, port: +port || 5390, extraIps: extra.split(',').map((s: string) => s.trim()).filter(Boolean), alt: alt.trim(), secret: secret.trim() || null, shares, enabled };
    try {
      const r = await api('POST', 'peers', body);
      await load();
      if (r.secret) { made = r.secret; toast('Сосед добавлен', 'ok'); }
      else { closeModal(); toast('Сохранено', 'ok'); }
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function del() { if (await removePeer(p)) closeModal(); }
</script>

<h3>Сосед «{p.title || p.name}»</h3>
<label class="f">Имя узла — сосед сообщает его сам<input disabled value={p.name + (p.pending ? ' (временное — до первого ответа)' : '')} /></label>
<label class="f">Псевдоним — для вас (можно менять)<input bind:value={title} placeholder="Рабочий ноутбук" /></label>
<div class="row"><label class="f" style="flex:1">Основной адрес (IP) — Radmin, домашняя сеть<input bind:value={ip} placeholder="192.168.1.172 или 26.x.x.x" /></label><label class="f" style="width:110px">Порт<input bind:value={port} /></label></div>
<div class="row" style="align-items:flex-end"><label class="f" style="flex:1">Запасной адрес — если основной не отвечает. Адрес соседа в своей сети (10.77.x.y) мост вписывает сам, когда сосед его сообщит{#if netIp && !alt} — сейчас это <b class="mono">{netIp}</b>{/if}<input bind:value={alt} placeholder={netIp || '10.77.0.x'} /></label>
  <button class="b sm" style="margin-bottom:10px" disabled={!alt.trim()} title="Поменять местами: запасной станет основным, основной — запасным" onclick={swapAlt}>⇅ Сделать основным</button></div>
<label class="f">Ещё адреса, с которых его пускать (через запятую). С 1.4 сосед сообщает свои адреса сам — вписывать нужно только для старых версий<input bind:value={extra} /></label>
<label class="f">Ключ пары (пусто — не менять; новый — такой же и на том ПК)<input bind:value={secret} placeholder="BRS-…" autocomplete="off" /></label>
<div class="f">Пространства<div class="row">{#each ui.ov.shares as s (s.id)}<label><input type="checkbox" value={s.id} bind:group={shares} /> {s.id}</label>{/each}</div>
  <div class="small muted">Что принимать от соседа в пространстве — правила «accept» в bridge.json (по умолчанию ничего).</div></div>
<label class="f" style="flex-direction:row;align-items:center;gap:8px;color:var(--text);font-size:14px"><input type="checkbox" bind:checked={enabled} /> Включён — мост связывается с этим соседом (снимите, чтобы временно отключить)</label>
{#if made}
  <p><b>Создан секрет пары</b> — впишите его соседу (peers → secret). Он показывается один раз:</p><div class="secret">{made}</div>
  <div class="row" style="margin-top:6px"><button class="b sm" onclick={() => copy(made, toast)}>Скопировать</button></div>
{/if}
<div class="row" style="margin-top:12px">
  <button class="b danger" onclick={del}><Gi n="trash" /> Удалить соседа</button><div class="grow"></div>
  <button class="b" onclick={closeModal}>{made ? 'Готово' : 'Отмена'}</button>
  {#if !made}<button class="b pri" onclick={save}>Сохранить</button>{/if}
</div>
