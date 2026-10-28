<script lang="ts">
  // Показы в канале: свои (кто смотрит, остановить) и чужие (смотреть, отдельное окно, во весь экран, пометки, управление).
  import { titleOf, SET } from '../lib/state.svelte';
  import { popover } from '../lib/pop';
  import { R, V, rosterOf, watch, popout, shareStop, dismissEnded } from './engine.svelte';
  import { SX, setMode, clearMine, release, decorate, streamVol, setStreamVol, scrKeys, keyName } from './screen.svelte';
  import { setSink } from '../lib/audio';
  import Gi from '../components/Gi.svelte';
  import CtlAskPop from './pops/CtlAskPop.svelte';

  const data = $derived.by(() => {
    void V.v;
    const people = rosterOf(R.key);
    const mine = Object.entries(R.mine).map(([id, S]: [string, any]) => ({ id, label: S.label, stream: S.stream, viewers: people.filter(p => (p.st.watching || []).includes(id)).map(p => titleOf(p.name)) }));
    const list: any[] = [];
    for (const p of people) for (const s of p.st.streams || []) list.push({ n: p.name, ...s, watching: R.watching.has(s.id), media: R.remote[s.id] || null, popped: !!R.popups[s.id] });
    const ended = Object.entries(R.ended).map(([id, e]: [string, any]) => ({ id, ...e, popped: !!R.popups[id] }));
    return { mine, list, ended };
  });
  function src(node: HTMLVideoElement, s: MediaStream | null) {
    const set = (x: MediaStream | null) => { if (node.srcObject !== x) { node.srcObject = x; node.muted = node.dataset.mine ? true : R.deaf; if (!node.dataset.mine && node.dataset.sid) node.volume = streamVol(node.dataset.sid); node.play().catch(() => {}); if (!node.dataset.mine) setSink(node); } };
    set(s);
    return { update: set };
  }
  let res = $state<Record<string, string>>({});
  const volOf = (id: string) => (void SET.svol, void V.v, streamVol(id));
  // во весь экран — карточка целиком: заголовок, инструменты (пинг, рисование, управление) и видео
  let fsId = $state('');
  function full(id: string) {
    if (document.fullscreenElement) { document.exitFullscreen().catch(() => {}); return; }
    (document.querySelector('.stream[data-stream="' + CSS.escape(id) + '"]') as HTMLElement | null)?.requestFullscreen?.().catch(() => {});
  }
  $effect(() => {
    const f = () => { const el = document.fullscreenElement as HTMLElement | null; fsId = el?.classList.contains('stream') ? el.dataset.stream || '' : ''; };
    document.addEventListener('fullscreenchange', f);
    return () => document.removeEventListener('fullscreenchange', f);
  });
</script>

<div>
  {#each data.mine as m (m.id)}
    <div class="share-row">
      <video class="mini" autoplay muted playsinline data-mine="1" use:src={m.stream}></video>
      <div class="grow"><b>{m.label}</b><div class="small muted">{#if m.viewers.length}<Gi n="eye" /> смотрят: {m.viewers.join(', ')}{:else}пока никто не смотрит — видео не передаётся{/if}</div></div>
      <button class="b sm danger" onclick={() => shareStop(m.id)}><Gi n="stop" /> Остановить</button>
    </div>
  {/each}
</div>
<div class="empty" class:hide={data.list.length > 0 || data.ended.length > 0}>Никто ничего не показывает</div>
<div class="streams">
  {#each data.ended as e (e.id)}
    <div class="stream ended" data-stream={e.id}>
      <div class="row"><b><Gi n="screen" /> {titleOf(e.n)}</b><span class="small muted">{e.label}</span></div>
      <div class="endbox">
        <div class="endt">Показ закончился</div>
        <div class="small muted">{e.popped ? 'Окно показа тоже ждёт. ' : ''}Если {titleOf(e.n)} снова начнёт показ, он откроется здесь сам.</div>
        <button class="b pri" style="min-width:140px" onclick={() => dismissEnded(e.id)}>Закрыть</button>
      </div>
    </div>
  {/each}
  {#each data.list as s (s.id)}
    {@const mode = SX.mode[s.id] || ''}
    {@const ctlMine = SX.cv.stream === s.id}
    {@const K = scrKeys()}
    <div class="stream" data-stream={s.id}>
      <div class="row"><b><Gi n="screen" /> {titleOf(s.n)}</b><span class="small muted">{s.label}</span><div class="grow"></div>
        {#if s.watching}
          {#if s.media}
            <button class="b sm" title="открыть в отдельном окне — его можно перетащить на другой монитор" onclick={() => popout(s.id, titleOf(s.n) + ': ' + s.label)}><Gi n="pop" /> Окно</button>
            <button class="b sm" title={fsId === s.id ? 'Свернуть (Esc)' : 'Во весь экран — с инструментами'} onclick={() => full(s.id)}><Gi n="expand" />{fsId === s.id ? ' Свернуть' : ''}</button>
          {/if}
          <button class="b sm" onclick={() => watch(s.id, false)}>✕ Не смотреть</button>
        {:else}
          <button class="b sm pri" onclick={() => watch(s.id, true)}><Gi n="eye" /> Смотреть</button>
        {/if}
      </div>
      {#if s.watching && s.popped}
        <div class="empty">Открыт в отдельном окне</div>
      {:else if s.watching && s.media}
        <div class="stools">
          <button class="b sm ib" class:on={mode === 'ping'} title={'Указка: щелчок по экрану — пинг, его видят все (' + keyName(K.ping) + ')'} onclick={() => setMode(s.id, 'ping')}><svg class="gi" viewBox="0 0 24 24"><circle cx="12" cy="12" r="3" /><circle cx="12" cy="12" r="8" /></svg></button>
          <button class="b sm ib" class:on={mode === 'draw'} title={'Карандаш: рисовать поверх экрана — видят все, через несколько секунд исчезает (' + keyName(K.draw) + ')'} onclick={() => setMode(s.id, 'draw')}><Gi n="board" /></button>
          <button class="b sm ib" class:on={mode === 'erase'} title={'Стёрка: проведите по своей линии (' + keyName(K.erase) + ')'} onclick={() => setMode(s.id, 'erase')}><svg class="gi" viewBox="0 0 24 24"><path d="M16 4l4 4-9 9H7l-3-3z" /><path d="M11 17h9" /></svg></button>
          <button class="b sm ib" title="Стереть все свои пометки" onclick={() => clearMine(s.id)}><svg class="gi" viewBox="0 0 24 24"><path d="M8 20h12M5 15l9-9 5 5-8 8H8z" /></svg></button>
          <i class="dsep" style="height:22px"></i>
          <label class="svol" title={'Громкость звука показа: ' + Math.round(volOf(s.id) * 100) + '% (запоминается для ' + titleOf(s.n) + ')'}><Gi n={volOf(s.id) === 0 ? 'mute' : 'sound'} /><input type="range" min="0" max="1" step="0.05" value={volOf(s.id)} oninput={e => setStreamVol(s.id, +(e.target as HTMLInputElement).value)} /><em>{Math.round(volOf(s.id) * 100)}%</em></label>
          <i class="dsep" style="height:22px"></i>
          {#if ctlMine}
            <span class="pill red">вы управляете · {SX.cv.keyboard ? 'мышь и клавиатура' : 'мышь'}</span>
            <button class="b sm" class:on={mode === 'ctl'} onclick={() => setMode(s.id, 'ctl')}>{mode === 'ctl' ? 'Управляю' : 'Взять управление'}</button>
            <button class="b sm danger" onclick={() => release(true)}>Отпустить</button>
          {:else}
            <button class="b sm" disabled={s.surf !== 'monitor'} title={s.surf === 'monitor' ? 'Попросить управление его ПК' : 'Управлять можно, только когда показывают весь экран'}
              onclick={e => popover(e.currentTarget as Element, 'ctlask', CtlAskPop, { id: s.id, n: s.n })}><svg class="gi" viewBox="0 0 24 24"><rect x="6" y="3" width="12" height="18" rx="6" /><path d="M12 7v4" /></svg> Управление…</button>
          {/if}
        </div>
        <div class="sv">
          <video class="rv screen-view" autoplay playsinline data-sid={s.id} use:src={s.media} ondblclick={() => { if (!SX.mode[s.id]) full(s.id); }} onresize={e => { const v = e.currentTarget as HTMLVideoElement; res[s.id] = v.videoWidth + '×' + v.videoHeight; }}></video>
          <canvas class="ann" data-mode={mode} use:decorate={s}></canvas>
        </div>
        <div class="small muted vstat">{res[s.id] || ''}</div>
      {:else if s.watching}
        <div class="empty">Соединяюсь…</div>
      {/if}
    </div>
  {/each}
</div>

<style>
  .endbox { min-height: 220px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 12px; text-align: center; border-radius: 12px; background: rgba(0,0,0,.35); margin-top: 8px; padding: 20px; }
  .endt { font-size: 18px; font-weight: 600; }
</style>
