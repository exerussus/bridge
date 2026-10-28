<script lang="ts">
  // Настройки приложения (3.9), как в Android: права переключателями, постоянное разрешение, автозапуск,
  // подписи без вопроса, данные, остановка, отзыв. Меняет только хозяин ПК — приложения сюда доступа не имеют.
  import { closeModal, toast, openModal } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { size, ago, fullTime } from '../lib/util';
  import { AP, PERM_LABEL, loadApps, openApp, closeApp, type AppInfo } from '../lib/apps.svelte';
  import AppConsent from './AppConsent.svelte';
  let { id }: { id: string } = $props();
  const a = $derived(AP.list.find(x => x.id === id) as AppInfo | undefined);
  const c = $derived(a?.consent || null);
  let busy = $state('');
  const hue = (s: string) => { let h = 0; for (const ch of s) h = (h * 31 + ch.charCodeAt(0)) >>> 0; return h % 360; };

  async function set(body: any, what: string) {
    busy = what;
    try { await api('POST', 'apps/settings', { id, ...body }); await loadApps(); } catch (e: any) { toast(e.message, 'bad'); await loadApps(); }
    busy = '';
  }
  function perm(p: string, on: boolean) {
    if (!a) return;
    const next = on ? [...new Set([...a.granted, p])] : a.granted.filter(x => x !== p);
    set({ perms: next }, 'perm:' + p);
  }
  async function call(path: string, body: any, ok: string) {
    try { await api('POST', path, body); toast(ok, 'ok'); await loadApps(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function revoke() {
    if (!a || !confirm('Отозвать разрешение «' + a.name + '»? Оно остановится, автозапуск выключится; открыть снова — только после нового разрешения.')) return;
    closeApp(a.id); await call('apps/revoke', { id }, 'Разрешение отозвано');
  }
  async function clearData() {
    if (!a || !confirm('Стереть данные «' + a.name + '» (' + size(a.dataSize) + ')? Сохранения и настройки приложения уйдут в корзину моста.')) return;
    await call('apps/data/clear', { id }, 'Данные очищены');
  }
  async function remove() {
    if (!a || !confirm('Убрать «' + a.name + '» в корзину моста?')) return;
    closeApp(a.id); closeModal(); await call('apps/remove', { id }, 'Убрано в корзину моста');
  }
  const running = $derived(!!(a?.runningService || a?.runningNative));
</script>

{#if a}
  <div class="row" style="gap:14px;align-items:center">
    <span class="appic big" style="--h:{hue(a.id)}">{#if a.icon}<img src={'/apps/' + encodeURIComponent(a.id) + '/icon'} alt="" />{:else}{a.name.slice(0, 2)}{/if}</span>
    <div class="grow" style="min-width:0">
      <div style="font-size:20px;font-weight:600">{a.name} <span class="muted" style="font-size:14px;font-weight:400">{a.version}</span></div>
      <div class="small muted">{a.publisher ? 'издатель ' + a.publisherFp : 'без подписи издателя'} · {a.packaged ? 'пакет' : 'папка'}{a.author ? ' · ' + a.author : ''}</div>
    </div>
    <span class="row" style="gap:6px">
      <button class="b sm pri" style="min-width:96px" disabled={a.state !== 'ok'} onclick={() => { closeModal(); openApp(a!.id); }}>Открыть</button>
      <button class="b sm" style="min-width:110px" disabled={!running} onclick={() => { if (a?.runningService) call('apps/stop', { id, kind: 'service' }, 'Служба остановлена'); if (a?.runningNative) call('apps/stop', { id, kind: 'native' }, 'Окно закрыто'); }}>Остановить</button>
    </span>
  </div>

  {#if a.state !== 'ok'}
    <div class="pill warn" style="display:block;margin-top:12px;line-height:1.5">{a.state === 'changed' ? 'Приложение изменилось' + (a.changeWhy ? ': ' + a.changeWhy : '') + ' — нужно разрешить заново.' : a.state === 'new' ? 'Приложение ещё не разрешено.' : 'Не для этой системы: ' + a.unsupported}
      {#if a.state !== 'unsupported'}<button class="b sm pri" style="margin-left:8px" onclick={() => openModal(AppConsent, { id })}>Разрешить…</button>{/if}</div>
  {:else}
    <div class="sec">
      <div class="plbl">Разрешение</div>
      <label class="srow"><span class="grow"><b>Постоянное разрешение</b><span class="small muted">{c?.trust ? 'Обновления ' + (a.publisher ? 'этого издателя' : 'этой папки') + ' — без вопроса, если не просят новых прав и адресов' : 'При каждом обновлении мост спросит снова'}</span></span>
        <input type="checkbox" class="sw" checked={!!c?.trust} disabled={busy === 'trust'} onchange={e => set({ trust: (e.target as HTMLInputElement).checked }, 'trust')} /></label>
      {#if c?.trust && !a.publisher}<div class="small" style="color:var(--warn)">Без подписи: любое изменение файлов приложения на этом ПК примется без вопроса.</div>{/if}
      <div class="small muted">Разрешено {ago(c?.at)} (версия {c?.version}){c?.updated ? ' · обновлено без вопроса ' + ago(c.updated) : ''}</div>
    </div>

    {#if a.service || a.native}
      <div class="sec">
        <div class="plbl">Запускать вместе с мостом</div>
        {#if a.service}
          <label class="srow"><span class="grow"><b>Фоновая служба</b><span class="small muted">{a.serviceAlways ? 'Работает всегда — так задано самим приложением' : 'Работает всё время, пока работает мост; упадёт — поднимется сама'}{a.runningService ? ' · сейчас работает' : ''}</span></span>
            <input type="checkbox" class="sw" checked={a.serviceAlways || !!c?.autostart?.service} disabled={a.serviceAlways || busy === 'svc'} onchange={e => set({ autostart: { service: (e.target as HTMLInputElement).checked } }, 'svc')} /></label>
        {/if}
        {#if a.native}
          <label class="srow"><span class="grow"><b>Своё окно</b><span class="small muted">Открывается при запуске моста (закроете — до следующего запуска не откроется){a.runningNative ? ' · сейчас открыто' : ''}</span></span>
            <input type="checkbox" class="sw" checked={!!c?.autostart?.native} disabled={busy === 'nat'} onchange={e => set({ autostart: { native: (e.target as HTMLInputElement).checked } }, 'nat')} /></label>
        {/if}
      </div>
    {/if}

    <div class="sec">
      <div class="plbl">Права</div>
      {#each a.perms as p (p)}
        <label class="srow"><span class="grow">{PERM_LABEL[p] || p}</span><input type="checkbox" class="sw" checked={a.granted.includes(p)} disabled={busy === 'perm:' + p} onchange={e => perm(p, (e.target as HTMLInputElement).checked)} /></label>
      {:else}<div class="small muted">Особых прав не просит.</div>{/each}
      {#if a.network?.length}<div class="small muted" style="margin-top:4px">Ходит в интернет: <span class="mono">{a.network.join(', ')}</span></div>{/if}
    </div>

    {#if c?.signAllowed?.length}
      <div class="sec">
        <div class="plbl">Подписи вашим ключом без вопроса</div>
        {#each c.signAllowed as p (p)}
          <div class="srow"><span class="grow mono">{p}</span><button class="b sm" onclick={() => set({ signRevoke: p }, 'sign')}>Спрашивать снова</button></div>
        {/each}
      </div>
    {/if}
  {/if}

  <div class="sec">
    <div class="plbl">Данные</div>
    <div class="srow"><span class="grow">Хранилище и файлы приложения на этом ПК: <b>{size(a.dataSize || 0)}</b></span><button class="b sm" disabled={!a.dataSize} onclick={clearData}>Очистить</button></div>
  </div>

  <div class="row" style="margin-top:14px;gap:8px">
    <button class="b danger" onclick={remove}>Убрать</button>
    {#if a.state === 'ok'}<button class="b" onclick={revoke}>Отозвать разрешение</button>{/if}
    <div class="grow"></div><button class="b" onclick={closeModal}>Закрыть</button>
  </div>
{:else}
  <div class="empty">Приложение убрано</div>
  <div class="row"><div class="grow"></div><button class="b" onclick={closeModal}>Закрыть</button></div>
{/if}

<style>
  .sec { margin-top: 14px; padding-top: 10px; border-top: 1px solid var(--line); }
  .srow { display: flex; align-items: center; gap: 12px; padding: 6px 0; cursor: pointer; }
  .srow > span:first-child { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .sw { appearance: none; width: 40px; height: 22px; border-radius: 11px; background: var(--panel2, #1b1d38); border: 1px solid var(--line); position: relative; cursor: pointer; flex: none; transition: background .15s; }
  .sw::after { content: ''; position: absolute; top: 2px; left: 2px; width: 16px; height: 16px; border-radius: 50%; background: #A3A7CC; transition: transform .15s, background .15s; }
  .sw:checked { background: rgba(56,225,240,.25); border-color: var(--acc, #38E1F0); }
  .sw:checked::after { transform: translateX(18px); background: var(--acc, #38E1F0); }
  .sw:disabled { opacity: .5; cursor: default; }
</style>
