<script lang="ts">
  // (3.21) Верхняя строка на всех вкладках: микрофон и звук (быстро заглушиться), обмен, уведомления, тема.
  // Звук выключен → микрофон тоже красный и перечёркнутый (пунктир — «выключен вместе со звуком»).
  import { ui, toast, load, SET, setSet, titleOf } from '../lib/state.svelte';
  import { api, IS_APP } from '../lib/api';
  import { R, V, toggleMic, toggleDeaf } from '../voice/engine.svelte';
  import Gi from './Gi.svelte';

  const st = $derived.by(() => {
    void V.v;
    const deaf = !!R.deaf, forced = R.forced as string, micOff = deaf || !R.mic || !!forced || !R.voice;
    return { deaf, forced, micOff, inRoom: !!R.key, title: R.title as string, voice: !!R.voice };
  });
  const text = $derived(
    st.forced ? 'вас заглушил ' + titleOf(st.forced)
    : st.deaf ? 'звук и микрофон выключены'
    : st.micOff ? 'микрофон выключен'
    : st.inRoom ? 'в канале «' + st.title + '»' : '');
  const micTitle = $derived(st.forced ? 'Вас заглушил ' + titleOf(st.forced) + ' — нажмите, чтобы снова говорить' : st.deaf ? 'Микрофон выключен вместе со звуком — нажмите, чтобы включить оба' : st.micOff ? 'Включить микрофон' : 'Выключить микрофон');

  let syncing = $state(false);
  async function sync() { syncing = true; try { await api('POST', 'sync'); toast('Обмен выполнен', 'ok'); await load(); } catch (e: any) { toast('Обмен: ' + e.message, 'bad'); } syncing = false; }
  async function toggleNotify() {
    if (IS_APP) { setSet('notify', !SET.notify); toast(SET.notify ? 'Уведомления включены' : 'Уведомления выключены'); return; }
    if (!('Notification' in window)) { toast('Браузер не поддерживает уведомления', 'bad'); return; }
    if (!SET.notify && Notification.permission !== 'granted') { const p = await Notification.requestPermission(); if (p !== 'granted') { toast('Уведомления запрещены в браузере', 'bad'); return; } }
    setSet('notify', !SET.notify);
    toast(SET.notify ? 'Уведомления включены' : 'Уведомления выключены');
  }
  function theme() { const next = (document.documentElement.dataset.theme || 'dark') === 'dark' ? 'light' : 'dark'; document.documentElement.dataset.theme = next; setSet('theme', next); }
</script>

<div class="tbar">
  {#if ui.conn}<span class="pill bad">{ui.conn}</span>{/if}
  <span class="tb-state ell" class:red={st.micOff || st.deaf} title={text}>{text}</span>
  <div class="tb-pair">
    <button class="tb-vo" class:mute={st.micOff && !(st.deaf && !st.forced)} class:forced={st.deaf && !st.forced} aria-pressed={st.micOff} aria-label={micTitle} title={micTitle + ' (Ctrl M)'} onclick={toggleMic}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="9" y="3" width="6" height="11" rx="3" /><path d="M5 11a7 7 0 0 0 14 0M12 18v3" />{#if st.micOff}<path d="M4 4l16 16" />{/if}</svg>
    </button>
    <button class="tb-vo" class:mute={st.deaf} aria-pressed={st.deaf} aria-label={st.deaf ? 'Включить звук' : 'Выключить звук и микрофон'} title={(st.deaf ? 'Включить звук' : 'Выключить звук и микрофон') + ' (Ctrl D)'} onclick={toggleDeaf}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 15v-3a8 8 0 0 1 16 0v3" /><rect x="3.5" y="14" width="4" height="6.5" rx="1.5" /><rect x="16.5" y="14" width="4" height="6.5" rx="1.5" />{#if st.deaf}<path d="M4 4l16 16" />{/if}</svg>
    </button>
  </div>
  <button class="b" title="Обменяться со всеми соседями сейчас (S)" disabled={syncing} onclick={sync}><Gi n="sync" /> Обменяться</button>
  <button class="b sq" style="opacity:{SET.notify ? 1 : 0.5}" aria-label="Уведомления" title={SET.notify ? 'Уведомления включены (нажмите, чтобы выключить)' : 'Включить уведомления о звонках, сообщениях и задачах'} onclick={toggleNotify}><Gi n="bell" /></button>
  <button class="b sq" aria-label="Светлая или тёмная тема" title="Светлая / тёмная тема" onclick={theme}><Gi n="theme" /></button>
</div>
