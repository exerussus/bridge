<script lang="ts">
  import { closeModal, load, openModal, toast, ui } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import EditPeer from './EditPeer.svelte';
  let { ip = '', title = '' }: { ip?: string; title?: string } = $props();
  let addr = $state(ip);
  let key = $state('');
  let ttl = $state(title && !/^new-/.test(title) ? title : '');
  let busy = $state(false);
  let out = $state<{ kind: string; r?: any; err?: string } | null>(null);
  const own = $derived((ui.ov.announce || []).map((u: string) => u.replace('http://', '')));
  function split() {
    const v = addr.trim().split(/\s+/);
    if (v.length === 2 && /^BRS-/.test(v[1]) && !key) { addr = v[0]; key = v[1]; }
  }
  async function go() {
    busy = true; out = { kind: 'wait' };
    try {
      const r = await api('POST', 'peers/connect', { address: addr.trim(), secret: key.trim() || null, title: ttl.trim() || null });
      await load();
      out = { kind: r.online ? 'ok' : 'saved', r };
      if (r.online) toast('Сосед «' + r.name + '» подключён', 'ok');
    } catch (e: any) { out = { kind: 'err', err: e.message }; }
    busy = false;
  }
  function focus(n: HTMLInputElement) { n.focus(); }
</script>

<h3>Подключить соседа</h3>
<div class="small" style="background:var(--accent-soft);padding:8px 10px;border-radius:7px;margin-bottom:10px">Нужны только <b>адрес</b> того ПК и <b>ключ</b> пары — имя сосед сообщит сам. Строку «адрес ключ» из приглашения можно вставить целиком в поле адреса.</div>
<label class="f">Адрес соседа (можно с портом) или строка «адрес ключ»<input bind:value={addr} oninput={split} use:focus placeholder="26.12.34.56  или  192.168.1.20:5390  или  26.12.34.56:5390 BRS-…" autocomplete="off" /></label>
<label class="f">Ключ пары<input bind:value={key} placeholder="BRS-…" autocomplete="off" /></label>
<label class="f">Псевдоним — для вас (необязательно)<input bind:value={ttl} placeholder="ПК технолога" /></label>
<div class="small muted">Ключа нет? Его создаёт тот, кто приглашает: на одном из ПК «Пригласить». Адрес этого ПК для соседа: {#each own as a}<span class="tag">{a}</span> {:else}—{/each}</div>
<div style="margin-top:10px">
  {#if out?.kind === 'wait'}<div class="small muted">Стучусь к соседу…</div>
  {:else if out?.kind === 'ok'}<div class="pill ok">✓ на связи</div> <b>Сосед назвался «{out.r.name}»</b>{out.r.updated ? ' — адрес обновлён' : ''}<div class="small muted" style="margin-top:6px">Дальше — что с ним делить: пространства и общие папки.</div>
  {:else if out?.kind === 'saved'}<div><span class="pill warn">сохранён, ждём</span> {out.r.error}</div>{#if out.r.hint}<div class="small muted" style="margin-top:4px">{out.r.hint}</div>{/if}
    <div class="small" style="margin-top:6px">Связь появится сама, как только причина уйдёт (например, на том ПК введут этот же ключ). Подробно — «Проверить» у соседа.</div>
  {:else if out?.kind === 'err'}<div class="pill bad">{out.err}</div>{/if}
</div>
<div class="row" style="margin-top:12px"><div class="grow"></div>
  <button class="b" onclick={closeModal}>Отмена</button>
  {#if out?.kind === 'ok'}<button class="b pri" onclick={() => openModal(EditPeer, { name: out!.r.name })}>Выбрать пространства</button>
  {:else if out?.kind === 'saved'}<button class="b pri" onclick={closeModal}>Готово</button>
  {:else}<button class="b pri" onclick={go} disabled={busy}>Подключить</button>{/if}
</div>
