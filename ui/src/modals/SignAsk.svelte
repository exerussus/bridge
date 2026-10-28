<script lang="ts">
  // Приложение просит подписать строку вашим ключом: один раз, всегда (запомнится в настройках приложения) или нет.
  import { closeModal, toast } from '../lib/state.svelte';
  import { api } from '../lib/api';
  let { m, done }: { m: any; done: () => void } = $props();
  let busy = $state(false);
  // свойства окна читаются лениво — после закрытия их уже нет, поэтому берём сразу
  const ask = m.ask, fin = done;
  async function answer(ok: boolean, always = false) {
    busy = true;
    try { await api('POST', 'apps/identity/answer', { ask, ok, remember: ok && !always, always }); }
    catch (e: any) { toast(e.message, 'bad'); }
    closeModal(); fin();
  }
</script>

<h3 style="margin:0">«{m.name}» просит подписать вашим ключом</h3>
<div class="small muted" style="margin:6px 0 10px">{m.publisher ? 'Издатель ' + m.publisherFp + ' — подпись пакета проверена.' : 'Приложение без подписи издателя.'} Подпись докажет его серверу, что это вы. Мост подставит в неё имя приложения, издателя и время.</div>
<div class="kv"><span class="muted">Назначение</span><b class="mono">{m.purpose}</b></div>
<div class="kv"><span class="muted">Данные</span><span class="mono small sgprev">{m.preview || '—'}</span></div>
<div class="row" style="margin-top:14px;gap:8px;flex-wrap:wrap">
  <button class="b danger" disabled={busy} onclick={() => answer(false)}>Не подписывать</button>
  <div class="grow"></div>
  <button class="b" disabled={busy} title="До перезапуска моста такие подписи этого приложения — без вопроса" onclick={() => answer(true)}>Разрешить сейчас</button>
  <button class="b pri" disabled={busy} title="Всегда без вопроса; снять — в настройках приложения" onclick={() => answer(true, true)}>Разрешать всегда</button>
</div>

<style>
  .kv { display: grid; grid-template-columns: 110px 1fr; gap: 10px; padding: 4px 0; align-items: start; }
  .sgprev { white-space: pre-wrap; word-break: break-all; max-height: 140px; overflow: auto; }
</style>
