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
  let secret = $state('');
  let shares = $state<string[]>([...p.shares]);
  let enabled = $state(p.enabled);
  let made = $state('');
  async function save() {
    const body: any = { name: p.name, title, ip: ip.trim() || null, port: +port || 5390, extraIps: extra.split(',').map((s: string) => s.trim()).filter(Boolean), secret: secret.trim() || null, shares, enabled };
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
<label class="f">Псевдоним — для вас (можно менять)<input bind:value={title} placeholder="ПК технолога" /></label>
<div class="row"><label class="f" style="flex:1">Адрес (IP)<input bind:value={ip} placeholder="192.168.1.172 или 26.x.x.x" /></label><label class="f" style="width:110px">Порт<input bind:value={port} /></label></div>
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
