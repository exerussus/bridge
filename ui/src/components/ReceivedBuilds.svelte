<script lang="ts">
  // Присланные сборки (3.8): что прислали соседи — последняя версия, история патчей с заметками и запуск программы.
  import { toast, peerTitle, SET, setSet } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, size, fullTime } from '../lib/util';
  import Gi from './Gi.svelte';
  let { B, reload }: { B: any; reload: () => void } = $props();
  let busy = $state('');
  let open = $state<Record<string, boolean>>({});
  const key = (r: any) => r.peer + '/' + r.name;

  async function run(r: any) {
    const k = key(r) + '|' + r.exe;
    const ok = (SET.buildRunOk || []) as string[];
    if (!ok.includes(k)) {
      if (!confirm('Запустить «' + r.exe + '» из сборки «' + r.name + '» от ' + peerTitle(r.peer) + '?\n\nЭто программа с другого ПК: она получит те же возможности, что любая программа на вашем компьютере. Запускайте, если доверяете отправителю.')) return;
      setSet('buildRunOk', [...ok, k].slice(-100));
    }
    busy = key(r);
    try { const x = await api('POST', 'builds/run', { peer: r.peer, name: r.name }); toast(x.already ? 'Уже запущено' : 'Запущено: ' + r.exe, 'ok'); reload(); } catch (e: any) { toast(e.message, 'bad'); }
    busy = '';
  }
  async function stop(r: any) {
    busy = key(r);
    try { await api('POST', 'builds/stop', { peer: r.peer, name: r.name }); toast('Остановлено'); reload(); } catch (e: any) { toast(e.message, 'bad'); }
    busy = '';
  }
  async function setTarget(r: any) {
    const v = prompt('Куда распаковывать сборку «' + r.name + '» от ' + peerTitle(r.peer) + '?\nПолный путь; пусто — по умолчанию (' + B.defaultRoot + ').', r.target);
    if (v === null) return;
    try { await api('POST', 'builds/target', { peer: r.peer, name: r.name, path: v.trim() }); toast('Следующая сборка распакуется сюда', 'ok'); reload(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function openDir(r: any) { try { await api('POST', 'open?' + new URLSearchParams({ target: 'build', peer: r.peer, name: r.name })); } catch (e: any) { toast(e.message, 'bad'); } }
  const last = (r: any) => r.lastOk || r;
</script>

{#each B.received as r (key(r))}
  {@const L = last(r)}
  <div class="card rb">
    <div class="row rbh">
      <span class="rbic"><Gi n="box" /></span>
      <div class="grow" style="min-width:0">
        <div class="row" style="gap:8px"><b class="rbn">{r.name}</b>{#if L.n}<span class="chip">версия №{L.n}</span>{/if}<span class="small muted">от {peerTitle(r.peer)}</span></div>
        <div class="small muted" title={fullTime(L.at)}>обновлена {ago(L.at)} · {L.mode === 'clean' ? 'целиком' : 'патч'} · {L.files} {L.files === 1 ? 'файл' : 'файлов'} · {size(L.bytes || 0)}{r.error ? ' · последняя не распаковалась' : ''}</div>
      </div>
      <div class="row rbact">
        {#if r.exe}
          <span class="rbrun">
            {#if r.running}
              <span class="pill good">запущено</span><button class="b sm danger" disabled={busy === key(r)} onclick={() => stop(r)}>Остановить</button>
            {:else}
              <button class="b pri" style="width:100%" disabled={busy === key(r) || !r.exeExists} title={r.exeExists ? r.exe : 'нет файла ' + r.exe + ' в папке сборки'} onclick={() => run(r)}>▶ Запустить</button>
            {/if}
          </span>
        {/if}
        <button class="b sm" onclick={() => openDir(r)}><Gi n="folder" /> Папка</button>
        <button class="b sm" onclick={() => setTarget(r)}>Куда…</button>
      </div>
    </div>
    {#if L.note}<div class="rbnote"><span class="plbl" style="margin:0">Что нового</span><div>{L.note}</div></div>{/if}
    {#if r.error}<div class="small" style="color:var(--warn);margin-top:6px">Последняя присланная не распаковалась: {r.error}</div>{/if}
    <div class="small muted mono rbt">{r.target}{r.exe ? ' · запуск: ' + r.exe : ''}</div>
    {#if r.history.length > 1}
      <button class="b sm" style="margin-top:8px" onclick={() => (open[key(r)] = !open[key(r)])}>{open[key(r)] ? 'Скрыть историю' : 'История версий · ' + r.history.length}</button>
      {#if open[key(r)]}
        <div class="rbh2">
          {#each r.history as h, i (h.at + i)}
            <div class="brow"><div class="row small" style="gap:8px">
              {#if h.n}<span class="chip">№{h.n}</span>{/if}
              {#if h.error}<span style="color:var(--warn)">не распаковалась: {h.error}</span>{:else}<span class="chip">{h.mode === 'clean' ? 'целиком' : 'патч'}</span><span class="muted">{h.files} {h.files === 1 ? 'файл' : 'файлов'} · {size(h.bytes || 0)}</span>{/if}
              <div class="grow"></div><span class="muted" title={fullTime(h.at)}>{ago(h.at)}</span></div>
              {#if h.note}<div class="small rbhn">{h.note}</div>{/if}
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
{:else}
  <div class="card"><div class="empty">Вам пока ничего не присылали. Когда сосед отправит сборку, она появится здесь: последняя версия, что нового и кнопка запуска.</div></div>
{/each}

<style>
  .rb { margin-bottom: 12px; }
  .rbh { gap: 12px; align-items: center; }
  .rbic { width: 42px; height: 42px; border-radius: 12px; display: grid; place-items: center; background: rgba(56,225,240,.12); color: var(--acc, #38E1F0); flex: none; }
  .rbn { font-size: 16px; }
  .rbact { gap: 8px; flex-wrap: wrap; justify-content: flex-end; }
  .rbrun { width: 210px; display: inline-flex; gap: 8px; align-items: center; justify-content: flex-end; }
  .rbnote { margin-top: 10px; padding: 8px 10px; border-radius: 10px; background: var(--panel2); white-space: pre-wrap; display: flex; flex-direction: column; gap: 4px; }
  .rbt { margin-top: 6px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .rbh2 { margin-top: 6px; }
  .rbhn { white-space: pre-wrap; color: var(--muted, #A3A7CC); padding: 2px 0 0 4px; }
  .brow { padding: 6px 0; border-top: 1px solid var(--line); }
</style>
