<script lang="ts">
  import { ui, toast, SET, setSet } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { pickFiles } from '../lib/util';
  import { PING_KINDS, playPing, sfx, testTone } from '../lib/audio';
  import { AV, loadAvatars, playJoinSound, remic, setMicGain, applyOut } from '../voice/engine.svelte';
  import { avatarPick, avatarRemove, keyName } from '../lib/avatar';
  import { me } from '../lib/state.svelte';
  import Seg from '../components/Seg.svelte';
  import Av from '../components/Av.svelte';
  import TileEditor from './TileEditor.svelte';

  const TABS: [string, string][] = [['tile', 'Плитка и аватарка'], ['audio', 'Звук и микрофон'], ['signals', 'Сигналы и пинг'], ['share', 'Показ экрана'], ['board', 'Доска'], ['look', 'Вид']];
  let mics = $state<MediaDeviceInfo[]>([]);
  let outs = $state<MediaDeviceInfo[]>([]);
  let labels = $state(true);
  async function devices() {
    try {
      const d = await navigator.mediaDevices.enumerateDevices();
      mics = d.filter(x => x.kind === 'audioinput' && x.deviceId !== 'default');
      outs = d.filter(x => x.kind === 'audiooutput' && x.deviceId !== 'default');
      labels = d.some(x => x.label);
    } catch {}
  }
  $effect(() => { devices(); });
  const pct = (v: number) => Math.round(v * 100) + '%';
  const sinkOk = !!(HTMLMediaElement.prototype as any).setSinkId;

  // проверка микрофона: уровень с текущей громкостью и обработкой
  let mt = $state({ on: false, lvl: 0, hot: false });
  let MT: any = {};
  async function micTest() {
    if (mt.on) { stopTest(); return; }
    try { MT.s = await navigator.mediaDevices.getUserMedia({ audio: { deviceId: SET.mic ? { exact: SET.mic } : undefined, echoCancellation: SET.ec, noiseSuppression: SET.ns, autoGainControl: SET.agc } }); }
    catch (e: any) { toast('Микрофон: ' + e.message, 'bad'); return; }
    MT.ac = new AudioContext();
    const src = MT.ac.createMediaStreamSource(MT.s); MT.g = MT.ac.createGain(); MT.g.gain.value = SET.micGain;
    const an = MT.ac.createAnalyser(); an.fftSize = 512; src.connect(MT.g).connect(an);
    const buf = new Uint8Array(an.fftSize);
    mt.on = true; devices();
    const tick = () => { if (!mt.on) return; an.getByteTimeDomainData(buf); let m = 0; for (const x of buf) m = Math.max(m, Math.abs(x - 128)); mt.lvl = Math.min(100, m / 1.28); mt.hot = m > 120; requestAnimationFrame(tick); };
    tick();
    setTimeout(() => { if (mt.on) stopTest(); }, 30000);
  }
  function stopTest() { mt.on = false; MT.s?.getTracks().forEach((t: MediaStreamTrack) => t.stop()); try { MT.ac?.close(); } catch {} mt.lvl = 0; }
  $effect(() => () => stopTest());

  let pttWait = $state(false);
  function pttSet() {
    pttWait = true;
    const h = (e: KeyboardEvent) => { e.preventDefault(); if (e.code !== 'Escape') setSet('pttKey', e.code); pttWait = false; document.removeEventListener('keydown', h, true); };
    document.addEventListener('keydown', h, true);
  }

  async function soundFile(kind: 'joinsound' | 'pingsound') {
    const [f] = await pickFiles({ accept: 'audio/*,.mp3,.ogg,.opus,.wav,.m4a,.webm' });
    if (!f) return;
    const max = kind === 'pingsound' ? 300 * 1024 : 1048576;
    if (f.size > max) { toast(kind === 'pingsound' ? 'Звук ' + Math.round(f.size / 1024) + ' КБ — нужен короткий, до 300 КБ' : 'Звук ' + (f.size / 1048576).toFixed(1) + ' МБ — нужен короткий, до 1 МБ (пара секунд)', 'bad'); return; }
    try {
      await api('POST', kind, f, true);
      await loadAvatars();
      if (kind === 'pingsound') { setSet('ping', 'custom'); playPing(AV); toast('Свой звук пинга сохранён', 'ok'); }
      else { playJoinSound(me(), true); toast('Звук входа сохранён — его услышат в канале, когда вы войдёте (играет до 6 с)', 'ok'); }
    } catch (e: any) { toast((kind === 'pingsound' ? 'Звук пинга: ' : 'Звук входа: ') + e.message, 'bad'); }
  }
  async function soundRemove(kind: 'joinsound' | 'pingsound') {
    try {
      await api('POST', kind + '/remove');
      if (kind === 'pingsound' && SET.ping === 'custom') setSet('ping', 'drop');
      await loadAvatars();
      toast(kind === 'pingsound' ? 'Свой звук пинга убран' : 'Звук входа убран — будет обычный сигнал', 'ok');
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  function setTheme(t: string) { setSet('theme', t); document.documentElement.dataset.theme = t; }
</script>

<div class="vtabs sttabs" role="tablist" aria-label="Разделы настроек" style="margin-bottom:14px">
  {#each TABS as [k, t]}<button class:on={ui.stTab === k} onclick={() => (ui.stTab = k)}>{t}</button>{/each}
</div>
<div class="grid">
  {#if ui.stTab === 'tile'}
    <TileEditor />
  {:else if ui.stTab === 'audio'}
    <div class="card"><h3>Микрофон</h3>
      <div class="setrow"><span>Устройство</span><select value={SET.mic} onchange={e => { setSet('mic', (e.target as HTMLSelectElement).value); remic(); }}>
        <option value="">по умолчанию</option>{#each mics as d}<option value={d.deviceId}>{d.label || 'микрофон'}</option>{/each}
        {#if !labels}<option disabled>названия появятся после первого доступа к микрофону — «Проверить»</option>{/if}
      </select><span></span></div>
      <div class="setrow"><span>Громкость</span><input type="range" min="0" max="2" step="0.05" value={SET.micGain} oninput={e => { const v = +(e.target as HTMLInputElement).value; setMicGain(v); setSet('micGain', v); if (MT.g) MT.g.gain.value = v; }} /><span class="val">{pct(SET.micGain)}</span></div>
      <div class="setrow"><span>Проверка</span><div class="lvl2"><i style="width:{mt.lvl}%;background:{mt.hot ? 'var(--red)' : 'var(--green)'}"></i></div><button class="b sm" onclick={micTest}>{mt.on ? 'Стоп' : 'Проверить'}</button></div>
      <div class="setrow"><span>Обработка</span><div class="row" style="gap:12px;flex-wrap:wrap">
        {#each [['ns', 'шумоподавление'], ['ec', 'эхоподавление'], ['agc', 'автоусиление']] as [k, t]}<label class="small"><input type="checkbox" checked={SET[k]} onchange={e => { setSet(k, (e.target as HTMLInputElement).checked); remic(); }} /> {t}</label>{/each}
      </div><span></span></div>
      <div class="setrow"><span>Рация: клавиша</span><span><span class="keycap">{pttWait ? 'нажмите клавишу…' : keyName(SET.pttKey)}</span></span><button class="b sm" onclick={pttSet}>Назначить</button></div>
      <div class="small muted">Изменения применяются сразу, в том числе посреди разговора.</div>
    </div>
    <div class="card"><h3>Звук собеседников</h3>
      <div class="setrow"><span>Устройство вывода</span><select value={SET.out} disabled={!sinkOk} onchange={e => { setSet('out', (e.target as HTMLSelectElement).value); applyOut(); }}>
        <option value="">по умолчанию</option>{#each outs as d}<option value={d.deviceId}>{d.label || 'динамики'}</option>{/each}
      </select><span></span></div>
      <div class="setrow"><span>Общая громкость</span><input type="range" min="0" max="1" step="0.05" value={SET.outVol} oninput={e => { setSet('outVol', +(e.target as HTMLInputElement).value); applyOut(); }} /><span class="val">{pct(SET.outVol)}</span></div>
      <div class="setrow"><span>Проверка</span><span class="small muted">короткий сигнал на выбранное устройство</span><button class="b sm" onclick={testTone}>Прослушать</button></div>
      <div class="small muted">Сигналы канала и пинг звучат на этом же устройстве.</div>
    </div>
  {:else if ui.stTab === 'signals'}
    <div class="card"><h3>🔔 Когда пингуют вас</h3>
      <div class="setrow"><span>Звук</span><select value={SET.ping} onchange={e => { setSet('ping', (e.target as HTMLSelectElement).value); playPing(AV); }}>
        {#each Object.entries(PING_KINDS) as [k, v]}<option value={k}>{v.t}</option>{/each}<option value="custom" disabled={!AV.ping}>свой файл</option>
      </select><button class="b sm" onclick={() => playPing(AV)}>Прослушать</button></div>
      <div class="setrow"><span>Свой файл</span><span class="row" style="gap:8px;flex-wrap:nowrap"><button class="b sm" onclick={() => soundFile('pingsound')}>Выбрать</button><button class="b sm" disabled={!AV.ping} onclick={() => soundRemove('pingsound')}>Убрать</button><span class="small muted" style="width:72px;white-space:nowrap">{AV.ping ? 'свой' : 'нет'}</span></span><span></span></div>
      <div class="setrow"><span>Громкость</span><input type="range" min="0" max="1" step="0.05" value={SET.pingVol} oninput={e => setSet('pingVol', +(e.target as HTMLInputElement).value)} onchange={() => playPing(AV)} /><span class="val">{pct(SET.pingVol)}</span></div>
      <div class="setrow"><span>Вспышка плитки</span><label class="small"><input type="checkbox" checked={SET.pingFlash} onchange={e => setSet('pingFlash', (e.target as HTMLInputElement).checked)} /> плитка того, кто пингует, вспыхивает</label><span></span></div>
      <div class="setrow"><span>Звук не чаще</span><Seg k="pingEvery" opts={[[1, 'раза в 1 с'], [3, 'в 3 с'], [10, 'в 10 с']]} /><span></span></div>
      <div class="setrow"><span>Не беспокоить</span><label class="small"><input type="checkbox" checked={SET.pingDnd} onchange={e => setSet('pingDnd', (e.target as HTMLInputElement).checked)} /> без звука — только вспышка</label><span></span></div>
      <div class="small muted">Свой звук слышите только вы. MP3, OGG, WAV или M4A до 300 КБ, играет до 3 секунд.</div>
    </div>
    <div class="card"><h3>Когда пингуете вы</h3>
      <div class="small muted">Когда вы пингуете кого-то, у вас звучит его звук пинга — тот, что он поставил себе. Остальные в канале ничего не слышат.</div>
    </div>
    <div class="card"><h3>Звуки канала</h3>
      <div class="setrow"><span>Звуки канала</span><label class="small"><input type="checkbox" checked={SET.sfx} onchange={e => { setSet('sfx', (e.target as HTMLInputElement).checked); if (SET.sfx) sfx('unmute'); }} /> вход, выход, микрофон, показ</label><span></span></div>
      <div class="setrow"><span>Их громкость</span><input type="range" min="0" max="1" step="0.05" value={SET.sfxVol} oninput={e => setSet('sfxVol', +(e.target as HTMLInputElement).value)} onchange={() => sfx('join')} /><span class="val">{pct(SET.sfxVol)}</span></div>
      <div class="setrow"><span>Мой звук входа</span><span class="row" style="gap:8px;flex-wrap:nowrap"><button class="b sm" onclick={() => soundFile('joinsound')}>Выбрать</button><button class="b sm" disabled={!AV.sound} onclick={() => soundRemove('joinsound')}>Убрать</button><span class="small muted" style="width:72px;white-space:nowrap" title={AV.sound ? 'Ваш звук слышат все в канале, когда вы входите' : 'Своего звука нет — играет обычный сигнал'}>{AV.sound ? 'свой' : 'обычный'}</span></span>
        <button class="b sm" onclick={() => { if (!playJoinSound(me(), true)) sfx('join'); }}>Прослушать</button></div>
      <div class="setrow"><span>Звуки входа других</span><label class="small"><input type="checkbox" checked={SET.joinSounds} onchange={e => setSet('joinSounds', (e.target as HTMLInputElement).checked)} /> играть, когда сосед входит в канал</label><span></span></div>
      <div class="setrow"><span>Их громкость</span><input type="range" min="0" max="1" step="0.05" value={SET.joinVol} oninput={e => setSet('joinVol', +(e.target as HTMLInputElement).value)} /><span class="val">{pct(SET.joinVol)}</span></div>
    </div>
    <div class="card"><h3>Сообщения</h3>
      <div class="setrow"><span>Личные сообщения</span><label class="small"><input type="checkbox" checked={SET.chatSound} onchange={e => setSet('chatSound', (e.target as HTMLInputElement).checked)} /> звук при новом</label><span></span></div>
      <div class="setrow"><span>Уведомления</span><span class="small muted">о звонках и сообщениях, когда окно свёрнуто — кнопка 🔔 вверху</span><span></span></div>
    </div>
  {:else if ui.stTab === 'share'}
    <div class="card"><h3>Показ экрана</h3>
      <div class="setrow"><span>Качество</span><Seg k="shareQ" opts={[['text', 'Чёткий текст · 30 к/с'], ['motion', 'Плавность · 60 к/с'], ['low', 'Экономно']]} /><span></span></div>
      <div class="setrow"><span>Звук системы</span><label class="small"><input type="checkbox" checked={SET.shareAudio} onchange={e => setSet('shareAudio', (e.target as HTMLInputElement).checked)} /> вместе с экраном</label><span></span></div>
      <div class="setrow"><span>Курсор</span><label class="small"><input type="checkbox" checked={SET.shareCursor} onchange={e => setSet('shareCursor', (e.target as HTMLInputElement).checked)} /> показывать указатель мыши</label><span></span></div>
      <div class="small muted">То же — в канале: «Показы» → ⚙.</div>
    </div>
  {:else if ui.stTab === 'board'}
    <div class="card"><h3>Доска</h3>
      <div class="setrow"><span>Синхронизация</span><Seg k="bSync" opts={[[16, 'сразу'], [50, '20/с'], [100, '10/с'], [250, '4/с']]} /><span></span></div>
      <div class="setrow"><span>Курсоры других</span><label class="small"><input type="checkbox" checked={SET.bCursors} onchange={e => setSet('bCursors', (e.target as HTMLInputElement).checked)} /> показывать</label><span></span></div>
      <div class="setrow"><span>Холст</span><Seg k="bGrid" opts={[['dots', 'точки'], ['grid', 'клетка'], ['none', 'пусто']]} /><span></span></div>
      <div class="setrow"><span>Привязка к сетке</span><label class="small"><input type="checkbox" checked={SET.bSnap} onchange={e => setSet('bSnap', (e.target as HTMLInputElement).checked)} /> фигуры, заметки и картинки</label><span></span></div>
      <div class="setrow"><span>Сглаживать штрихи</span><label class="small"><input type="checkbox" checked={SET.bSmooth} onchange={e => setSet('bSmooth', (e.target as HTMLInputElement).checked)} /> перо и маркер</label><span></span></div>
      <div class="setrow"><span>Картинки</span><Seg k="bImg" opts={[[0, 'как есть'], [2560, 'до 2560 px'], [1280, 'до 1280 px']]} /><span></span></div>
      <div class="small muted">Синхронизация — как часто ваши штрихи уходят остальным: реже — меньше трафика по слабой сети. Фон и слои доски — в самой доске (они общие для всех в канале).</div>
    </div>
  {:else}
    <div class="card"><h3>Вид</h3>
      <div class="setrow"><span>Тема</span><select value={SET.theme} onchange={e => setTheme((e.target as HTMLSelectElement).value)}><option value="dark">тёмная</option><option value="light">светлая</option></select><span></span></div>
      <div class="setrow"><span>Аватарка и плитка</span><span><button class="b sm" onclick={() => (ui.stTab = 'tile')}>Оформить плитку…</button></span><span></span></div>
      <div class="small muted">Настройки хранятся в мосте этого ПК (bridge-data/ui.json).</div>
    </div>
  {/if}
</div>
