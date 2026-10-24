<script lang="ts">
  import { closeModal, load, toast } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { copy } from '../lib/util';
  let title = $state('');
  let r = $state<any>(null);
  async function go() {
    try {
      r = await api('POST', 'peers/invite?' + new URLSearchParams({ title: title.trim() }));
      await load();
      if (r.warning) toast(r.warning, 'bad');
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  function focus(n: HTMLInputElement) { n.focus(); }
</script>

<h3>Пригласить соседа</h3>
<div class="small" style="background:var(--accent-soft);padding:8px 10px;border-radius:7px;margin-bottom:10px">Мост создаст ключ и строку <b>«адрес этого ПК + ключ»</b>. На том ПК её вставляют в «＋ Подключить» — и всё: имена и адреса ПК узнают друг у друга сами.</div>
<label class="f">Псевдоним — кто это будет (необязательно)<input bind:value={title} use:focus placeholder="Ноутбук Ильи" disabled={!!r} /></label>
{#if r}
  <p><b>Передайте на тот ПК эту строку</b> (лично или в надёжном мессенджере — ключ как пароль):</p>
  <div class="secret">{r.line}</div>
  <div class="row" style="margin-top:6px"><button class="b sm pri" onclick={() => copy(r.line, toast)}>Скопировать</button></div>
  {#if r.addresses.length > 1}
    <div class="small muted" style="margin-top:8px">Если тот ПК в другой сети (например, только Radmin) — с другим адресом этого ПК:</div>
    {#each r.addresses.slice(1) as a}
      <div class="row small" style="margin-top:4px"><span class="mono grow">{a + ' ' + r.secret}</span><button class="b sm" onclick={() => copy(a + ' ' + r.secret, toast)}>Скопировать</button></div>
    {/each}
  {/if}
  {#if !r.addresses.length}<div class="pill warn" style="margin-top:6px">мост слушает только 127.0.0.1 — добавьте свой адрес в «listen» (bridge.json), иначе соседу не достучаться</div>{/if}
  <div class="small muted" style="margin-top:8px">Там: «Соседи» → «＋ Подключить» → вставить строку. Здесь сосед появится сам, как только подключится. Ключ показывается один раз.</div>
{/if}
<div class="row" style="margin-top:12px"><div class="grow"></div>
  <button class="b" onclick={closeModal}>{r ? 'Готово' : 'Отмена'}</button>
  {#if !r}<button class="b pri" onclick={go}>Создать ключ</button>{/if}
</div>
