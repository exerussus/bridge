<script lang="ts">
  // Приглашение (3.21.1): сначала — по какому адресу этого ПК сосед будет подключаться. Мост показывает все свои сети
  // (Radmin, своя сеть, Tailscale, внешний, локальная) — лучший первым; можно вписать свой. Выбор запоминается.
  import { closeModal, load, toast, SET, setSet } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { copy } from '../lib/util';
  let title = $state('');
  let r = $state<any>(null);
  let choices = $state<{ addr: string; kind: string; listening: boolean }[]>([]);
  let pick = $state('');
  let own = $state('');
  let busy = $state(false);
  let loaded = $state(false);
  let stale = $state(''); // (3.22.1) запомненный адрес, которого у этого ПК больше нет
  const HINT: Record<string, string> = {
    Radmin: 'для тех, кто в той же сети Radmin',
    'своя сеть': 'для соседей в своей сети MostNet (через свой сервер)',
    Tailscale: 'для тех, кто в той же сети Tailscale',
    внешний: 'адрес в интернете — нужен проброс порта на роутере',
    'локальная сеть': 'только для ПК в этой же домашней сети (тот же роутер)',
  };
  $effect(() => {
    api('GET', 'peers/addresses').then(x => {
      choices = x.choices || [];
      const last = SET.inviteAddr as string;
      if (last && choices.some(c => c.addr === last)) pick = last;
      else {
        // прежнего адреса больше нет (сменился адрес Radmin, порт моста…) — не подставлять его молча, а взять лучший
        if (last) stale = last;
        pick = choices[0]?.addr || '_own';
      }
      loaded = true;
    }).catch(e => { loaded = true; toast(e.message, 'bad'); });
  });
  const chosen = $derived(pick === '_own' ? own.trim() : pick);
  const cur = $derived(choices.find(c => c.addr === chosen || c.addr.split(':')[0] === chosen));
  async function go() {
    if (!chosen) { toast('Выберите или впишите адрес этого ПК', 'bad'); return; }
    busy = true;
    try {
      r = await api('POST', 'peers/invite?' + new URLSearchParams({ title: title.trim(), address: chosen }));
      setSet('inviteAddr', r.line.split(' ')[0]);
      await load();
      if (r.warning) toast(r.warning, 'bad');
    } catch (e: any) { toast(e.message, 'bad'); }
    busy = false;
  }
  function focus(n: HTMLInputElement) { n.focus(); }
</script>

<h3>Пригласить соседа</h3>
<div class="small" style="background:var(--accent-soft);padding:8px 10px;border-radius:7px;margin-bottom:10px">Мост создаст ключ и строку <b>«адрес этого ПК + ключ»</b>. На том ПК её вставляют в «＋ Подключить» — и всё: остальные адреса ПК узнают друг у друга сами.</div>
<label class="f">Псевдоним — кто это будет (необязательно)<input bind:value={title} use:focus placeholder="Ноутбук Ильи" disabled={!!r} /></label>
{#if !r}
  <div class="f">По какому адресу этого ПК сосед подключится
    <div class="ich">
      {#if !loaded}<div class="ic ph"></div><div class="ic ph"></div>{/if}
      {#each choices as c (c.addr)}
        <label class="ic" class:on={pick === c.addr} class:dim={!c.listening}>
          <input type="radio" bind:group={pick} value={c.addr} />
          <span class="grow" style="min-width:0"><span class="mono">{c.addr}</span> <span class="pill plain {c.kind === 'локальная сеть' ? '' : 'accf'}">{c.kind}</span>
            <span class="small muted" style="display:block">{c.listening ? HINT[c.kind] || '' : 'мост на этом адресе пока не слушает — включите «listenAuto» или добавьте его в listen (bridge.json)'}</span></span>
        </label>
      {/each}
      <label class="ic" class:on={pick === '_own'}>
        <input type="radio" bind:group={pick} value="_own" />
        <span class="grow" style="min-width:0">Свой адрес <span class="small muted">— например, запасной или внешний</span>
          <input class="mono" style="width:100%;margin-top:6px" bind:value={own} placeholder="26.12.34.56 или 26.12.34.56:5390" onfocus={() => (pick = '_own')} /></span>
      </label>
    </div>
  </div>
  <!-- строка-подсказка всегда на месте (постоянной высоты), меняется только текст — кнопки не прыгают -->
  <div class="small note" class:warnc={cur?.kind === 'локальная сеть' || (!!stale && pick !== '_own')}>{#if cur?.kind === 'локальная сеть'}Это адрес домашней сети: подключиться по нему смогут только ПК за тем же роутером. Для соседа издалека выберите Radmin, свою сеть или внешний адрес.{:else if stale && pick !== '_own'}Прежний адрес {stale} у этого ПК больше не найден — выбран лучший из нынешних. Если он всё же нужен — впишите его в «Свой адрес».{/if}</div>
{:else}
  <p><b>Передайте на тот ПК эту строку</b> (лично или в надёжном мессенджере — ключ как пароль):</p>
  <div class="secret">{r.line}</div>
  <div class="row" style="margin-top:6px"><button class="b sm pri" onclick={() => copy(r.line, toast)}>Скопировать</button></div>
  {#if r.addresses.length > 1}
    <details style="margin-top:8px"><summary class="small muted" style="cursor:pointer">Строка с другим адресом этого ПК ›</summary>
      {#each r.addresses.slice(1) as a}
        <div class="row small" style="margin-top:4px"><span class="mono grow">{a + ' ' + r.secret}</span><button class="b sm" onclick={() => copy(a + ' ' + r.secret, toast)}>Скопировать</button></div>
      {/each}
    </details>
  {/if}
  <div class="small muted" style="margin-top:8px">Там: «Соседи» → «＋ Подключить» → вставить строку. Здесь сосед появится сам, как только подключится. Ключ показывается один раз.</div>
{/if}
<div class="row" style="margin-top:12px"><div class="grow"></div>
  <button class="b" onclick={closeModal}>{r ? 'Готово' : 'Отмена'}</button>
  {#if !r}<button class="b pri" disabled={busy || !chosen} onclick={go}>Создать ключ</button>{/if}
</div>

<style>
  .ich { display: flex; flex-direction: column; gap: 6px; margin-top: 6px; }
  .ic { display: flex; gap: 10px; align-items: flex-start; padding: 10px 12px; border-radius: 10px; border: 1px solid var(--line2); cursor: pointer; color: var(--text); font-weight: 500; font-size: 13px; }
  .ic.on { border-color: var(--cyan); background: var(--accent-soft); }
  .ic.dim { opacity: 0.65; }
  .ic input[type='radio'] { margin-top: 3px; }
  .ic.ph { height: 58px; box-sizing: border-box; background: var(--line2); opacity: 0.35; cursor: default; }
  .note { min-height: 3.2em; margin-top: -4px; color: var(--muted); }
  .note.warnc { color: var(--warn); }
</style>
