// ═════════════ Каналы: голос, экран, звонки ═════════════
// Канал — комната «владелец:id». Участник раз в 5 с и при каждом изменении рассылает участникам канала своё
// состояние: голос, микрофон, звук, свои показы экрана и чьи показы смотрит. Так все видят, кто где и что делает,
// даже не заходя. С каждым участником — одно соединение («вежливые переговоры»): звук — если голос включён у обоих,
// видео показа — только тем, кто его смотрит. Медиа идут напрямую между ПК; узел передаёт только сигналы.
//
// Движок — обычные объекты (соединения, потоки), а интерфейсу о переменах говорит счётчик V.v.
import { api, onMedia } from '../lib/api';
import { ui, toast, setTab, SET, titleOf, me as ME, badgeCount } from '../lib/state.svelte';
import { TILES } from '../lib/tile.svelte';
import { sfx, sfxDiff, ring, playFile, pingSound, playPing, PING_KINDS, setSink, setSilent } from '../lib/audio';
import { notifyTo } from '../lib/notify';
import { CHT } from '../lib/chat.svelte';

export const V = $state({ v: 0 });
export const bump = () => { V.v++; };
/** Кто говорит (обновляется 8 раз в секунду). */
export const SPK = $state<Record<string, boolean>>({});
/** Вспышка плитки при пинге: имя → номер вспышки. */
export const PINGED = $state<Record<string, number>>({});
/** Кто сейчас «здоровается» (играет его звук входа) — его плитка мигает, пока звук не кончится. */
export const GREET = $state<Record<string, boolean>>({});
/** Вид комнаты: вкладка (участники, показы, доска) и подвкладка показов. */
export const VV = $state({ view: 'people', sub: 'list', chatPinned: false });
/** Аватарки и свои звуки (свои и соседей): хэши файлов. */
export const AV = $state<{ me: string; peers: Record<string, string>; sound: string; sounds: Record<string, string>; ping: string; pings: Record<string, string>; tile: string; tiles: Record<string, string>; raw?: string; silent?: boolean; status?: any }>({ me: '', peers: {}, sound: '', sounds: {}, ping: '', pings: {}, tile: '', tiles: {} });
/** Входящие звонки и просьбы показать экран — плашки над каналами. */
export const INV = $state<{ id: string; media: string; text: string; from: string; ts: number; accept: () => void; decline: () => void }[]>([]);

export const R: any = {
  key: null as string | null, title: '', members: [] as string[], voice: true, mic: true, deaf: false, local: null as MediaStream | null,
  pcs: {} as Record<string, any>, mine: {} as Record<string, any>, remote: {} as Record<string, MediaStream>, watching: new Set<string>(), popups: {} as Record<string, Window>,
  /** (3.8) Показы, которые смотрели и которые закончились: окно и карточка остаются с плашкой «Показ закончился». */
  ended: {} as Record<string, { n: string; label: string; surf: string; at: number }>,
  /** Что известно о показах в канале (кто, подпись) — чтобы помнить и после их конца. */
  seen: {} as Record<string, { n: string; label: string; surf: string }>,
  vol: {} as Record<string, number>, ptt: false, pttDown: false, meters: {} as Record<string, () => number>, forced: '', joinedAt: 0, ac: null as AudioContext | null,
};
(window as any).R = R; // отдельному окну показа (assets/popout.html) — оно берёт видео отсюда

export let CH: { me: string; own: any[]; all: any[] } = { me: '', own: [], all: [] };
export const ROSTER: Record<string, Record<string, { st: any; t: number }>> = {};
export const ADHOC: Record<string, any> = {};
export const SESSION = Math.random().toString(36).slice(2, 10);
const RTC_CFG: RTCConfiguration = { iceServers: [], bundlePolicy: 'max-bundle' };

export const roomKey = (c: any) => c.owner + ':' + c.id;
export const roomDef = (k: string) => CH.all.find(c => roomKey(c) === k) || ADHOC[k];
export const isChannel = (k: string | null) => !!k && !!CH.all.find(c => roomKey(c) === k);
export function rosterOf(k: string | null) {
  const r = (k && ROSTER[k]) || {}, now = Date.now();
  return Object.entries(r).filter(([n, v]) => now - v.t < 16000 && n !== ME()).map(([n, v]) => ({ name: n, st: v.st }));
}
export const stOf = (n: string) => { const v = (ROSTER[R.key] || {})[n]; return v && Date.now() - v.t < 16000 ? v.st : null; };
function liveStreams() { const ids = new Set<string>(); for (const p of rosterOf(R.key)) for (const s of p.st.streams || []) ids.add(s.id); return ids; }
export function myState() {
  const live = liveStreams();
  return {
    sid: SESSION, pk: SET.ping || 'drop', fm: R.forced || '', voice: R.voice, mic: R.voice && R.mic && !R.forced && !R.deaf && !(R.ptt && !R.pttDown), deaf: R.deaf, ptt: R.ptt,
    micId: R.local ? R.local.id : '', streams: Object.entries(R.mine).map(([id, s]: [string, any]) => ({ id, label: s.label, surf: s.surf || '' })),
    watching: [...R.watching].filter((id: string) => live.has(id)), title: R.title,
    // вошёл тихо: остальные не играют звук входа (флаг живёт первые секунды)
    qj: R.quiet && Date.now() - (R.joinedAt || 0) < 20000 ? 1 : undefined,
    // (3.3.4) клиент сам говорит, что он только что вошёл (fr: 1) или уже сидит в канале (fr: 0): если после обрыва
    // связи он появился снова с fr: 0 — у остальных не приветствие, а сигнал «связь восстановлена»
    fr: Date.now() - (R.joinedAt || 0) < 20000 ? 1 : 0,
    // (3.3.4) статус — сразу всем в канале, не дожидаясь обмена с соседями
    sts: AV.status || null,
  };
}
export const onlinePeers = () => (ui.ov ? ui.ov.peers : []).filter((p: any) => p.enabled && !p.invite && !p.pending);

// ── сигналы ──
export async function signal(to: string, msg: any) { return api('POST', 'media/signal', Object.assign({ to }, msg)); }
const SENDQ: Record<string, Promise<any>> = {};
/** Сообщения одному собеседнику уходят строго по очереди (описание сессии раньше своих ICE-кандидатов). */
export function rsend(to: string, msg: any, room?: string) {
  const body = Object.assign({ media: 'room', room: room || R.key }, msg);
  const p = (SENDQ[to] || Promise.resolve()).catch(() => {}).then(() => signal(to, body));
  SENDQ[to] = p.catch(() => {});
  return p;
}
export const reachable = (n: string) => { const p = ui.ov && ui.ov.peers.find((x: any) => x.name === n); return !p || p.online || !p.lastSeen; };

let stateT: any = 0;
export function broadcastState() {
  if (!R.key) return;
  const st = myState();
  (ROSTER[R.key] = ROSTER[R.key] || {})[ME()] = { st, t: Date.now() };
  for (const n of R.members) if (n !== ME() && reachable(n)) rsend(n, { type: 'state', st }).catch(() => {});
}
/** Изменения состояния копятся 150 мс и уходят одним сообщением. */
export function stateChanged() { clearTimeout(stateT); stateT = setTimeout(() => { broadcastState(); bump(); }, 150); bump(); }
setInterval(() => { if (R.key) broadcastState(); bump(); }, 5000);

async function loadIce() { try { const r = await api('GET', 'media/ice'); RTC_CFG.iceServers = r.iceServers || []; } catch {} }
setInterval(loadIce, 300000);

export async function loadChannels() {
  try {
    const r = await api('GET', 'channels');
    CH = r;
    for (const p of r.presence || []) {
      const box = (ROSTER[p.room] = ROSTER[p.room] || {});
      if (!box[p.node] || box[p.node].t < Date.now() - p.age * 1000) box[p.node] = { st: p.st, t: Date.now() - p.age * 1000 };
    }
  } catch {}
  bump();
}

// ── аватарки и свои звуки: своя хранится в мосте, соседские мост забирает у них ──
export async function loadAvatars() {
  try {
    const r = await api('GET', 'avatars');
    const raw = JSON.stringify(r);
    if (raw === AV.raw) return;
    setSilent(!!r.silent);
    const stChanged = JSON.stringify(AV.status || null) !== JSON.stringify(r.status || null);
    Object.assign(AV, { raw, silent: !!r.silent, status: r.status || null, me: r.me || '', peers: r.peers || {}, sound: r.sound || '', sounds: r.sounds || {}, ping: r.ping || '', pings: r.pings || {}, tile: r.tile || '', tiles: r.tiles || {} });
    if (stChanged && R.key) broadcastState(); // свой статус — в канал сразу
  } catch {}
}
setInterval(loadAvatars, 30000);
export function avatarUrl(n: string): string {
  const mine = n === ME(), h = mine ? AV.me : AV.peers[n];
  return h ? 'avatar?peer=' + encodeURIComponent(mine ? '' : n) + '&v=' + h : '';
}

/** Звук входа в канал: свой задаётся в «Настройках», соседские мост забирает у них. false — сыграть обычный сигнал. */
export function playJoinSound(n: string, test = false) {
  const mine = n === ME(), h = mine ? AV.sound : AV.sounds[n];
  if (!h) return false;
  if (!test && (!SET.sfx || (!mine && !SET.joinSounds))) return false;
  const a = playFile('joinsound?peer=' + encodeURIComponent(mine ? '' : n) + '&v=' + h, SET.joinVol, 6);
  if (a && !test) {
    // плитка того, чей звук звучит, мигает до конца звука (не дольше 6 с)
    GREET[n] = true;
    const off = () => { delete GREET[n]; };
    a.addEventListener('ended', off, { once: true }); a.addEventListener('pause', off, { once: true }); a.addEventListener('error', off, { once: true });
    setTimeout(off, 6500);
  }
  return !!a;
}

// ── пинг ──
const PINGIN: Record<string, number> = {};
function pingIn(n: string) {
  if (SET.pingFlash !== false) pingFlash(n);
  if (SET.pingDnd) return;
  if (Date.now() - (PINGIN[n] || 0) < (+SET.pingEvery || 1) * 1000) return;
  PINGIN[n] = Date.now();
  playPing(AV);
}
/** Тот, кто пингует, слышит звук пинга того, кого пингует (его выбор); остальные — ничего. */
function pingAck(n: string) {
  const st = stOf(n) || {}, k = st.pk || 'drop';
  if (k === 'custom' && AV.pings && AV.pings[n]) { playFile('pingsound?peer=' + encodeURIComponent(n) + '&v=' + AV.pings[n], SET.pingVol, 3, () => pingSound('drop')); return; }
  pingSound(PING_KINDS[k] ? k : 'drop');
}
export function pingFlash(n: string) { PINGED[n] = (PINGED[n] || 0) + 1; }
const PINGT: Record<string, number> = {};
/** Статус участника: из состояния канала (сразу), иначе из его плитки. null — без статуса. */
export function statusOf(n: string): any {
  if (n === ME()) return AV.status || null;
  for (const box of Object.values(ROSTER)) { const e = (box as any)[n]; if (e && e.st && 'sts' in e.st && Date.now() - e.t < 16000) return e.st.sts; }
  const tl = TILES[n];
  return tl && tl.style && (tl.style.stText || tl.style.stColor || tl.stImg || tl.style.stSilent) ? { text: tl.style.stText || '', color: tl.style.stColor || '', silent: !!tl.style.stSilent, img: tl.stImg || '' } : null;
}
export function ping(n: string) {
  if (Date.now() - (PINGT[n] || 0) < 1200) return;
  // у него «полная тишина» — пинг не прозвучит, не шлём
  if (statusOf(n)?.silent) { toast('У ' + titleOf(n) + ' полная тишина — пинг не прозвучит', 'warn'); return; }
  PINGT[n] = Date.now();
  pingAck(n); pingFlash(n);
  activity('ping', { to: n });
  rsend(n, { type: 'ping' }).catch(e => toast(e.message, 'bad'));
}

// ── «заглушили»: видно у значка у часов, на кнопке в панели задач и в панели ──
export async function forceMute(n: string) {
  try { await rsend(n, { type: 'mute' }); toast('Заглушили ' + titleOf(n) + ' — включить микрофон сможет только он сам', 'ok'); } catch (e: any) { toast(e.message, 'bad'); }
}
function muteIndicate(by: string) {
  api('POST', 'self/muted', { by: by || '' }).catch(() => {});
  document.body.classList.toggle('force-muted', !!by);
  ui.forcedBy = by || '';
}

// ── микрофон ──
async function micStream(): Promise<MediaStream> {
  const dev = SET.mic;
  const raw = await navigator.mediaDevices.getUserMedia({ audio: { deviceId: dev ? { exact: dev } : undefined, echoCancellation: SET.ec, noiseSuppression: SET.ns, autoGainControl: SET.agc, channelCount: 1 }, video: false });
  if (Math.abs(SET.micGain - 1) < 0.01) return raw;
  // громкость микрофона: усилитель WebAudio (исходный поток остаётся у результата — чтобы его выключить)
  const ac = new AudioContext(), src = ac.createMediaStreamSource(raw), g = ac.createGain(), dst = ac.createMediaStreamDestination();
  g.gain.value = SET.micGain;
  src.connect(g).connect(dst);
  try { await ac.resume(); } catch {}
  const out: any = dst.stream;
  out._raw = raw; out._ac = ac; out._gain = g;
  return out;
}
function stopMic(s: any) { if (!s) return; s.getTracks().forEach((t: MediaStreamTrack) => t.stop()); s._raw?.getTracks().forEach((t: MediaStreamTrack) => t.stop()); try { s._ac?.close(); } catch {} }
/** Пересоздать микрофон (сменили устройство или обработку) посреди разговора. */
export async function remic() {
  if (!R.local) return;
  try {
    const s = await micStream(), t = s.getAudioTracks()[0];
    for (const L of Object.values(R.pcs) as any[]) if (L.audio) L.audio.replaceTrack(t);
    stopMic(R.local); R.local = s; applyMic(); meter('__me', s);
  } catch (e: any) { toast('Микрофон: ' + e.message, 'bad'); }
}
let gainT: any = 0;
export function setMicGain(v: number) {
  SET.micGain = v;
  if (R.local?._gain) R.local._gain.gain.value = v;
  else { clearTimeout(gainT); gainT = setTimeout(remic, 400); }
}
export function applyOut() {
  document.querySelectorAll<HTMLAudioElement>('audio[id^=ra-]').forEach(a => { const n = a.id.slice(3); a.volume = Math.min(1, (R.vol[n] ?? 1) * SET.outVol); setSink(a); });
  document.querySelectorAll<HTMLVideoElement>('video.rv').forEach(setSink);
}
export function setPeerVol(n: string, v: number) {
  R.vol[n] = v;
  const a = document.getElementById('ra-' + n) as HTMLAudioElement | null;
  if (a) a.volume = Math.min(1, v * SET.outVol);
  bump();
}

// ── вход и выход ──
/** Войти в канал. quiet — тихо: без звука входа у остальных и у себя. */
export async function roomJoin(key: string, quiet = false) {
  const voice = true;
  if (R.key === key) { setTab('voice'); return; }
  if (R.key) roomLeave();
  const c = roomDef(key);
  if (!c) { toast('Канал не найден — обновите список', 'bad'); return; }
  await loadIce();
  api('POST', 'media/local', { type: 'takeover', room: key, session: SESSION, dev: 'ПК' }).catch(() => {});
  Object.assign(R, { key, title: c.title, members: [...new Set([...c.members, ME()])], voice, mic: true, deaf: false, watching: new Set(), quiet });
  if (voice) {
    try { R.local = await micStream(); meter('__me', R.local!); }
    catch (e: any) { toast('Микрофон: ' + e.message + ' — вошли без микрофона (подключить снова — «Канал» → «Подключить микрофон»)', 'bad'); R.voice = false; }
  }
  R.joinedAt = Date.now();
  CHT.roomConv = 'ch:' + key;
  applyMic(); setTab('voice'); broadcastState();
  if (!quiet && !playJoinSound(ME())) sfx('join');
  for (const p of rosterOf(key)) syncLink(p.name);
  bump();
}
const leaveHooks: ((quiet: boolean) => void)[] = [];
export function onLeave(f: (quiet: boolean) => void) { leaveHooks.push(f); }
export function roomLeave(quiet = false) {
  if (!R.key) return;
  sfx('leave');
  // quiet — канал перешёл на другое устройство этого ПК: участникам «вышел» не слать
  if (!quiet) for (const n of R.members) if (n !== ME() && reachable(n)) rsend(n, { type: 'leave' }).catch(() => {});
  for (const n of Object.keys(R.pcs)) closeLink(n);
  for (const id of Object.keys(R.mine)) R.mine[id].stream.getTracks().forEach((t: MediaStreamTrack) => t.stop());
  for (const w of Object.values(R.popups) as Window[]) try { w.close(); } catch {}
  stopMic(R.local);
  if (ROSTER[R.key] && !quiet) delete ROSTER[R.key][ME()];
  if (R.forced) { R.forced = ''; muteIndicate(''); }
  for (const f of leaveHooks) try { f(quiet); } catch {}
  Object.assign(R, { key: null, title: '', members: [], local: null, pcs: {}, mine: {}, remote: {}, watching: new Set(), popups: {}, meters: {}, ended: {}, seen: {} });
  CHT.roomConv = '';
  document.querySelectorAll('audio[id^=ra-]').forEach(a => a.remove());
  bump();
}
addEventListener('beforeunload', () => roomLeave());

// ── микрофон, звук, рация ──
export function applyMic() {
  const on = R.voice && R.mic && !R.forced && !R.deaf && (!R.ptt || R.pttDown);
  R.local?.getAudioTracks().forEach((t: MediaStreamTrack) => (t.enabled = on));
  document.querySelectorAll<HTMLMediaElement>('audio[id^=ra-], video.rv').forEach(a => (a.muted = R.deaf));
}
addEventListener('keydown', e => {
  if (R.key && R.ptt && e.code === SET.pttKey && !(e.target as HTMLElement).matches('input,textarea,select')) {
    e.preventDefault();
    if (!R.pttDown) { R.pttDown = true; applyMic(); stateChanged(); }
  }
});
addEventListener('keyup', e => { if (R.key && R.ptt && e.code === SET.pttKey) { R.pttDown = false; applyMic(); stateChanged(); } });

function meter(name: string, stream: MediaStream) {
  try {
    R.ac = R.ac || new AudioContext();
    const src = R.ac.createMediaStreamSource(stream), an = R.ac.createAnalyser();
    an.fftSize = 512; src.connect(an);
    const buf = new Uint8Array(an.fftSize);
    R.meters[name] = () => { an.getByteTimeDomainData(buf); let m = 0; for (const x of buf) m = Math.max(m, Math.abs(x - 128)); return m; };
  } catch {}
}
export const micLevel = () => { const f = R.meters.__me; return f ? f() : 0; };
setInterval(() => {
  for (const [n, f] of Object.entries(R.meters) as [string, () => number][]) {
    const st = n === '__me' ? myState() : stOf(n);
    const on = f() > 12 && !!st && st.mic;
    if (!!SPK[n] !== on) SPK[n] = on;
  }
}, 120);

export async function toggleVoice() {
  if (R.voice) { R.voice = false; stopMic(R.local); R.local = null; delete R.meters.__me; }
  else {
    try { R.local = await micStream(); R.voice = true; R.mic = true; R.deaf = false; meter('__me', R.local!); }
    catch (e: any) { toast('Микрофон: ' + e.message, 'bad'); return; }
  }
  for (const n of Object.keys(R.pcs)) syncLink(n);
  for (const p of rosterOf(R.key)) syncLink(p.name);
  applyMic(); stateChanged();
}
export function toggleMic() {
  if (R.forced) { R.forced = ''; R.mic = true; R.deaf = false; muteIndicate(''); sfx('unmute'); applyMic(); stateChanged(); return; }
  const wasDeaf = R.deaf;
  R.mic = !R.mic;
  if (R.mic) R.deaf = false;
  sfx(wasDeaf && R.mic ? 'undeaf' : R.mic ? 'unmute' : 'mute');
  applyMic(); stateChanged();
}
export function toggleDeaf() { R.deaf = !R.deaf; sfx(R.deaf ? 'deaf' : 'undeaf'); applyMic(); stateChanged(); }
export function setPtt(on: boolean) { R.ptt = on; applyMic(); stateChanged(); }

// ── соединение с участником ──
function link(n: string) {
  if (R.pcs[n]) return R.pcs[n];
  const pc = new RTCPeerConnection(RTC_CFG);
  const L: any = (R.pcs[n] = { pc, id: Math.random().toString(36).slice(2, 9), rid: '', polite: ME() > n, making: false, ignore: false, audio: null, video: {}, early: [] });
  pc.onnegotiationneeded = async () => {
    try { L.making = true; await pc.setLocalDescription(); await rsend(n, { type: 'sdp', pc: L.id, desc: pc.localDescription!.toJSON() }); } catch {} finally { L.making = false; }
  };
  pc.onicecandidate = e => { if (e.candidate) rsend(n, { type: 'ice', pc: L.id, candidate: e.candidate.toJSON() }).catch(() => {}); };
  pc.ontrack = e => {
    const s = e.streams[0];
    if (!s) return;
    const st = stOf(n);
    if (e.track.kind === 'video' || (st && (st.streams || []).some((x: any) => x.id === s.id))) { R.remote[s.id] = s; e.track.onended = bump; bump(); return; }
    let a = document.getElementById('ra-' + n) as HTMLAudioElement | null;
    if (!a) { a = document.createElement('audio'); a.id = 'ra-' + n; a.autoplay = true; document.body.appendChild(a); }
    a.srcObject = s; a.muted = R.deaf; a.volume = Math.min(1, (R.vol[n] ?? 1) * SET.outVol); setSink(a); meter(n, s);
  };
  pc.onconnectionstatechange = () => {
    const cs = pc.connectionState;
    if (cs === 'failed' || cs === 'disconnected') L.badSince = L.badSince || Date.now();
    else if (cs === 'connected') L.badSince = 0;
    bump();
    if (cs === 'failed') { toast('Не удалось соединиться с ' + titleOf(n) + ' — проверьте связь (вкладка «Соседи» → «Проверить»)', 'bad'); try { pc.restartIce(); } catch {} }
  };
  return L;
}
// сторож: после встречных предложений Chrome иногда не начинает искать путь (ICE стоит в «new») — перезапустить поиск
setInterval(() => {
  for (const [n, L] of Object.entries(R.pcs) as [string, any][]) {
    const pc = L.pc;
    if (pc.localDescription && pc.remoteDescription && pc.signalingState === 'stable' && pc.iceConnectionState === 'new') {
      L.stuck = (L.stuck || 0) + 1;
      if (L.stuck === 2) { try { pc.restartIce(); } catch {} }
      else if (L.stuck >= 4) { closeLink(n); syncLink(n); }
    } else L.stuck = 0;
  }
}, 2000);
function closeLink(n: string) {
  const L = R.pcs[n];
  if (!L) return;
  try { L.pc.close(); } catch {}
  delete R.pcs[n]; delete R.meters[n];
  document.getElementById('ra-' + n)?.remove();
}
/** Что отправлять участнику: звук (если голос у обоих) и показы, которые он смотрит. */
export function syncLink(n: string) {
  if (!R.key || n === ME()) return;
  const st = stOf(n);
  if (!st) return;
  const wantAudio = !!(R.voice && R.local && st.voice);
  const wantVideo = Object.keys(R.mine).filter(id => (st.watching || []).includes(id));
  let L = R.pcs[n];
  if (!L && !wantAudio && !wantVideo.length) return;
  L = link(n);
  const track = R.local?.getAudioTracks()[0];
  if (wantAudio && !L.audio && track) {
    L.audio = L.pc.addTrack(track, R.local);
    const p = L.audio.getParameters(); p.encodings = [{ maxBitrate: 64000 }]; L.audio.setParameters(p).catch(() => {});
  } else if (wantAudio && L.audio && track && L.audio.track !== track) L.audio.replaceTrack(track);
  else if (!wantAudio && L.audio) { try { L.pc.removeTrack(L.audio); } catch {} L.audio = null; }
  for (const id of wantVideo) if (!L.video[id]) {
    const S = R.mine[id];
    L.video[id] = S.stream.getTracks().map((t: MediaStreamTrack) => L.pc.addTrack(t, S.stream));
    for (const s of L.video[id]) if (s.track.kind === 'video') {
      const p = s.getParameters(); p.degradationPreference = S.q.pref; p.encodings = [{ maxBitrate: S.q.br, maxFramerate: S.q.fps }]; s.setParameters(p).catch(() => {});
    }
  }
  for (const id of Object.keys(L.video)) if (!wantVideo.includes(id)) { for (const s of L.video[id]) try { L.pc.removeTrack(s); } catch {} delete L.video[id]; }
}
export function connState(n: string): { cs: string; bad: boolean } {
  const L = R.pcs[n];
  const state = L ? L.pc.connectionState : '';
  const bad = !!(L && L.badSince && Date.now() - L.badSince > 6000);
  const cs = state === 'connected' || ((state === 'disconnected' || state === 'new' || state === 'connecting') && !bad) ? ''
    : ({ connecting: 'соединяюсь…', new: 'соединяюсь…', failed: 'не соединилось', disconnected: 'связь прервалась' } as Record<string, string>)[state] ?? '';
  return { cs, bad };
}

// ── показ экрана: можно несколько сразу; видео уходит только смотрящим ──
export const QUAL: Record<string, { fps: number; br: number; pref: RTCDegradationPreference }> = {
  text: { fps: 30, br: 8e6, pref: 'maintain-resolution' }, motion: { fps: 60, br: 12e6, pref: 'maintain-framerate' }, low: { fps: 15, br: 1.5e6, pref: 'balanced' },
};
const shareHooks: ((id: string) => void)[] = [];
export function onShareStop(f: (id: string) => void) { shareHooks.push(f); }
export async function shareStart() {
  if (!R.key) { toast('Сначала зайдите в канал'); return; }
  const qn = QUAL[SET.shareQ] ? SET.shareQ : 'text', q = QUAL[qn];
  let s: MediaStream;
  try { s = await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: { ideal: q.fps, max: q.fps }, cursor: SET.shareCursor === false ? 'never' : 'always' } as any, audio: SET.shareAudio !== false }); }
  catch (e: any) { if (e.name !== 'NotAllowedError') toast('Показ экрана: ' + e.message, 'bad'); return; }
  const vt = s.getVideoTracks()[0];
  vt.contentHint = qn === 'motion' ? 'motion' : 'detail';
  const vs: any = vt.getSettings(), surf = vs.displaySurface, num = Object.keys(R.mine).length + 1;
  const nice = vt.label && !/^(screen|window|web-contents|tab)[:\-]/i.test(vt.label) ? ': ' + vt.label.slice(0, 40) : '';
  const label = (({ monitor: 'Экран', window: 'Окно', browser: 'Вкладка' } as Record<string, string>)[surf] || 'Экран') + nice + (num > 1 && !nice ? ' ' + num : '');
  R.mine[s.id] = { stream: s, label, q, qn, surf: surf || '', w: vs.width || 0, h: vs.height || 0 };
  sfx('share');
  vt.onended = () => shareStop(s.id);
  stateChanged();
}
export function shareStop(id: string) {
  const S = R.mine[id];
  if (!S) return;
  for (const f of shareHooks) try { f(id); } catch {}
  sfx('unshare');
  S.stream.getTracks().forEach((t: MediaStreamTrack) => t.stop());
  delete R.mine[id];
  for (const n of Object.keys(R.pcs)) syncLink(n);
  stateChanged();
}
export function watch(id: string, on: boolean) {
  if (on) R.watching.add(id);
  else { R.watching.delete(id); try { R.popups[id]?.close(); } catch {} delete R.popups[id]; delete R.remote[id]; }
  stateChanged();
}
/** Отдельное окно показа: видео на всё окно; двойной щелчок, F или кнопка — во весь экран. */
export function popout(id: string, label: string) {
  if (!R.remote[id]) return;
  const old = R.popups[id];
  if (old && !old.closed) { old.focus(); return; }
  const w = window.open('/assets/popout.html#' + encodeURIComponent(id + '|' + label), 'scr' + id.replace(/\W/g, ''), 'width=1100,height=650');
  if (!w) { toast('Не удалось открыть окно показа', 'bad'); return; }
  R.popups[id] = w;
  bump();
}
setInterval(() => {
  let changed = false;
  // окно закрыли — закончившийся показ забыт (кнопка «Закрыть» в окне или крестик)
  const live = R.key ? liveStreams() : new Set<string>();
  for (const [id, w] of Object.entries(R.popups) as [string, Window][]) if (!w || w.closed) {
    delete R.popups[id];
    // окно закончившегося показа закрыли (даже раньше, чем панель заметила конец) — показ забыт и в панели
    if (R.ended[id] || !live.has(id)) { delete R.ended[id]; if (R.watching.delete(id)) delete R.remote[id]; }
    changed = true;
  }
  if (changed) bump();
}, 1000);
function pruneWatching() {
  if (!R.key) return;
  const live = liveStreams();
  for (const p of rosterOf(R.key)) for (const s of p.st.streams || []) R.seen[s.id] = { n: p.name, label: s.label || '', surf: s.surf || '' };
  let ch = false;
  // смотрели, а показ закончился: не закрываем — окно и карточка показывают «Показ закончился» и кнопку «Закрыть»
  for (const id of [...R.watching] as string[]) if (!live.has(id) && !R.remote[id]?.active) {
    R.watching.delete(id); delete R.remote[id];
    const was = R.seen[id];
    if (was) R.ended[id] = { ...was, at: Date.now() };
    else { try { R.popups[id]?.close(); } catch {} delete R.popups[id]; }
    ch = true;
  }
  // тот же человек снова показывает — переключаемся на новый показ сами (окно остаётся тем же)
  for (const [eid, e] of Object.entries(R.ended) as [string, any][]) {
    const p = rosterOf(R.key).find(x => x.name === e.n);
    const cands = ((p && p.st.streams) || []).filter((s: any) => !R.watching.has(s.id) && !R.ended[s.id]);
    const s = cands.find((x: any) => x.label === e.label) || cands.find((x: any) => e.surf && x.surf === e.surf) || cands[0];
    if (!s) continue;
    R.watching.add(s.id);
    const w = R.popups[eid];
    delete R.popups[eid]; delete R.ended[eid];
    if (w && !w.closed) { try { w.location.hash = encodeURIComponent(s.id + '|' + titleOf(e.n) + ': ' + s.label); R.popups[s.id] = w; } catch {} }
    ch = true;
  }
  for (const id of Object.keys(R.seen)) if (!live.has(id) && !R.ended[id] && !R.watching.has(id)) delete R.seen[id];
  if (ch) stateChanged();
}
setInterval(pruneWatching, 1500);
/** Закрыть плашку закончившегося показа (и его окно). */
export function dismissEnded(id: string) {
  try { R.popups[id]?.close(); } catch {}
  delete R.popups[id]; delete R.ended[id];
  bump();
}
export function watchAllOf(n: string) {
  const st = stOf(n);
  for (const s of (st && st.streams) || []) if (!R.watching.has(s.id)) watch(s.id, true);
  VV.view = 'shows'; VV.sub = 'list';
}
export async function askScreen(n: string) {
  try { await rsend(n, { type: 'ask' }); toast('Попросили ' + titleOf(n) + ' показать экран'); } catch (e: any) { toast(e.message, 'bad'); }
}
export async function ringPeer(n: string) {
  try { await rsend(n, { type: 'invite', title: R.title, members: R.members }); toast('Зовём ' + titleOf(n)); } catch (e: any) { toast(e.message, 'bad'); }
}
export async function callPeers(names: string[], title?: string) {
  const k = ME() + ':call-' + Date.now().toString(36);
  ADHOC[k] = { owner: ME(), id: k.split(':')[1], title: title || 'Звонок: ' + [ME(), ...names].map(titleOf).join(', '), members: [ME(), ...names] };
  await roomJoin(k);
  const res: Record<string, string> = {};
  for (const n of names) {
    try { await rsend(n, { type: 'invite', title: ADHOC[k].title, members: ADHOC[k].members }, k); res[n] = 'звоним'; }
    catch (e: any) { res[n] = e.message; toast(titleOf(n) + ': ' + e.message, 'bad'); }
  }
  return res;
}

// ── входящие ──
function invite(id: string, media: string, text: string, from: string, ts: number, accept: () => void, decline: () => void) {
  if (INV.some(x => x.id === id)) return;
  INV.unshift({ id, media, text, from, ts, accept, decline });
}
export function dropInvite(id: string) { const i = INV.findIndex(x => x.id === id); if (i >= 0) INV.splice(i, 1); }
const setVoiceBadge = (v: number) => badgeCount('voice', v);

const msgHooks: ((m: any, n: string, k: string) => boolean)[] = [];
/** Свои сообщения канала (пометки на показе, управление ПК) — обработчик возвращает true, если сообщение его. */
export function onRoomMsg(f: (m: any, n: string, k: string) => boolean) { msgHooks.push(f); }

const RECVQ: Record<string, Promise<any>> = {};
let RECONT = 0;
function onRoomQueued(m: any) { const n = m.from || ''; RECVQ[n] = (RECVQ[n] || Promise.resolve()).then(() => onRoom(m)).catch(e => console.warn('room', e)); }
async function onRoom(m: any) {
  const k = m.room, n = m.from;
  if (m.type === 'state') {
    const box = (ROSTER[k] = ROSTER[k] || {});
    const isNew = !box[n] || Date.now() - box[n].t > 16000;
    // не входил заново, просто пропадала связь — так говорит сам клиент
    const reconn = isNew && !!m.st && m.st.fr === 0;
    const prev = box[n] && !isNew ? box[n].st : null;
    // участник сменил устройство (ПК ↔ телефон) — прежнее соединение больше не годится
    if (box[n] && box[n].st && box[n].st.sid && m.st && m.st.sid && box[n].st.sid !== m.st.sid) closeLink(n);
    box[n] = { st: m.st || {}, t: Date.now() };
    if (k === R.key && !R.deaf) {
      const d = reconn ? 'reconnect' : isNew ? 'join' : sfxDiff(prev, m.st);
      // сами только что вошли — все уже сидящие «новые», звуков нет
      if (d === 'reconnect') { if (Date.now() - (R.joinedAt || 0) > 4000 && Date.now() - RECONT > 1500) { RECONT = Date.now(); sfx('reconnect'); } } else
      // свой звук входа — только у тех, кто вошёл после нас (иначе при нашем входе заиграли бы все сразу)
      if (d === 'join' && m.st && m.st.qj) {} // вошёл тихо
      else if (d === 'join' && Date.now() - (R.joinedAt || 0) > 4000 && playJoinSound(n)) {} else if (d) sfx(d);
    }
    if (k === R.key) { if (!R.members.includes(n)) R.members.push(n); syncLink(n); if (reconn) { if (Date.now() - (R.joinedAt || 0) > 4000) toast('Связь с ' + titleOf(n) + ' восстановилась'); } else if (isNew) { toast(titleOf(n) + ' зашёл в «' + R.title + '»'); activity('channel.join', { room: k, who: n }); } }
    else if (isNew && !R.key && roomDef(k)) toast(titleOf(n) + ' в канале «' + roomDef(k).title + '» — вкладка «Каналы»');
    bump();
    return;
  }
  if (m.type === 'leave') {
    if (ROSTER[k]) delete ROSTER[k][n];
    if (k === R.key) {
      if (!R.deaf) sfx('leave');
      closeLink(n);
      for (const id of Object.keys(R.remote)) if (!R.remote[id].active) delete R.remote[id];
      toast(titleOf(n) + ' вышел');
    }
    dropInvite('inv-' + k); ring(false);
    bump();
    return;
  }
  if (m.type === 'invite') {
    if (R.key === k) return;
    ADHOC[k] = { owner: k.split(':')[0], id: k.split(':')[1], title: m.title || 'Звонок', members: m.members || [n] };
    ring(true); setVoiceBadge(1);
    if (ui.tab !== 'voice') toast(titleOf(n) + ' звонит — вкладка «Каналы»');
    if (document.hidden) notifyTo('Мост · звонок', titleOf(n) + ' звонит', 'voice');
    invite('inv-' + k, 'call', titleOf(n) + ' звонит' + ((m.members || []).length > 2 ? ' (участников: ' + m.members.length + ')' : ''), n, m.ts,
      () => { ring(false); setVoiceBadge(0); roomJoin(k); },
      () => { ring(false); setVoiceBadge(0); rsend(n, { type: 'decline' }, k).catch(() => {}); });
    return;
  }
  if (m.type === 'decline') { toast(titleOf(n) + ' не принял звонок'); return; }
  for (const h of msgHooks) if (h(m, n, k)) return;
  if (m.type === 'mute') {
    if (k === R.key && R.voice && !R.forced) {
      R.forced = n; R.mic = false; applyMic(); sfx('mute'); muteIndicate(n); stateChanged();
      toast('🔇 ' + titleOf(n) + ' заглушил вас — у вас, видимо, шумно. Включить микрофон можете только вы: кнопка «Микрофон»', 'bad');
      if (document.hidden) notifyTo('Мост · вас заглушили', titleOf(n) + ' заглушил ваш микрофон — включить можете только вы', 'voice');
    }
    return;
  }
  if (m.type === 'ping') { if (k === R.key) { pingIn(n); toast('🔔 ' + titleOf(n) + ' пингует'); } return; }
  if (m.type === 'ask') {
    ring(true); setVoiceBadge(1);
    invite('inv-' + k + '-ask-' + n, 'screen', titleOf(n) + ' просит показать экран', n, m.ts,
      () => { ring(false); setVoiceBadge(0); setTab('voice'); shareStart(); }, () => { ring(false); setVoiceBadge(0); });
    return;
  }
  if (k !== R.key) return;
  if (m.type === 'sdp') {
    // собеседник начал соединение заново (новый номер) — и мы с чистого листа
    let fresh = false;
    if (R.pcs[n] && m.pc && R.pcs[n].rid && R.pcs[n].rid !== m.pc && m.desc.type === 'offer') { closeLink(n); fresh = true; }
    const L = link(n), pc = L.pc, desc = m.desc;
    if (m.pc) L.rid = m.pc;
    const collision = desc.type === 'offer' && (L.making || pc.signalingState !== 'stable');
    L.ignore = !L.polite && collision;
    if (L.ignore) return;
    try {
      await pc.setRemoteDescription(desc);
      for (const c of L.early.splice(0)) try { await pc.addIceCandidate(c); } catch {}
      if (desc.type === 'offer') { await pc.setLocalDescription(); await rsend(n, { type: 'sdp', pc: L.id, desc: pc.localDescription.toJSON() }); if (fresh) syncLink(n); }
    } catch (e) { console.warn('sdp', e); }
  } else if (m.type === 'ice') {
    const L = link(n);
    if (m.pc && L.rid && m.pc !== L.rid) return;
    if (!L.pc.remoteDescription) { L.early.push(m.candidate); return; } // кандидат раньше описания — придержать
    try { await L.pc.addIceCandidate(m.candidate); } catch (e) { if (!L.ignore) console.warn('ice', e); }
  }
}

// ── пульт: команды ассистента через узел (POST /local/ui) ──
// Узел шлёт {media:'ui', type:'cmd', id, action, args}. Если открыто несколько окон панели — выполняет одно (ui/claim).
const PANEL_ID = Math.random().toString(36).slice(2), PANEL_T0 = Date.now();
const until = async <T>(f: () => T, ms: number): Promise<T> => { const t0 = Date.now(); while (Date.now() - t0 < ms) { const v = f(); if (v) return v; await new Promise(r => setTimeout(r, 150)); } return f(); };
export function roomSummary() {
  if (!R.key) return { room: null };
  return {
    room: R.key, title: R.title, voice: R.voice, mic: R.mic, deaf: R.deaf,
    members: rosterOf(R.key).map(p => Object.assign({ name: p.name, connected: R.pcs[p.name]?.pc.connectionState === 'connected' }, p.st)),
    mine: Object.entries(R.mine).map(([id, S]: [string, any]) => ({ id, label: S.label, viewers: rosterOf(R.key).filter(p => (p.st.watching || []).includes(id)).map(p => p.name) })),
    watching: [...R.watching], popouts: Object.keys(R.popups),
  };
}
async function findRoom(a: any) {
  await loadChannels();
  if (a.room && roomDef(a.room)) return a.room;
  const q = String(a.channel || a.room || '').toLowerCase();
  const c = CH.all.find(c => c.id.toLowerCase() === q || c.title.toLowerCase() === q) || CH.all.find(c => c.title.toLowerCase().includes(q));
  if (!c) throw new Error('нет такого канала: ' + (a.channel || a.room) + ' (есть: ' + CH.all.map(c => c.title).join(', ') + ')');
  return roomKey(c);
}
function findStream(a: any) {
  for (const p of rosterOf(R.key)) for (const s of p.st.streams || []) if (s.id === a.stream || (!a.stream && a.peer && p.name.toLowerCase() === String(a.peer).toLowerCase())) return { id: s.id, peer: p.name, label: s.label };
  throw new Error(a.stream ? 'нет такого показа' : a.peer ? titleOf(a.peer) + ' ничего не показывает' : 'нужен stream или peer');
}
const needRoom = () => { if (!R.key) throw new Error('сначала зайдите в канал (join) или позвоните (call)'); };
const UI: Record<string, (a: any) => Promise<any>> = {
  tab: async a => { setTab(a.tab); return { tab: ui.tab }; },
  state: async () => roomSummary(),
  join: async a => { const k = await findRoom(a); await roomJoin(k, !!a.quiet); await until(() => R.key === k, 5000); await new Promise(r => setTimeout(r, 1200)); return roomSummary(); },
  leave: async () => { roomLeave(); return { room: null }; },
  mic: async a => { needRoom(); if (!R.voice) throw new Error('голос выключен — сначала voice {on:true}'); R.mic = a.on !== false; if (R.mic) R.deaf = false; sfx(R.mic ? 'unmute' : 'mute'); applyMic(); stateChanged(); return roomSummary(); },
  deafen: async a => { needRoom(); R.deaf = a.on !== false; sfx(R.deaf ? 'deaf' : 'undeaf'); applyMic(); stateChanged(); return roomSummary(); },
  share: async a => {
    needRoom();
    if (a.quality && QUAL[a.quality]) SET.shareQ = a.quality;
    const before = new Set(Object.keys(R.mine));
    toast('Ассистент просит показать экран — выберите, что показать', 'ok');
    try { window.focus(); } catch {}
    await shareStart();
    const id = Object.keys(R.mine).find(x => !before.has(x));
    if (!id) throw new Error('показ не начался (выбор отменён или нет доступа к экрану)');
    return { id, label: R.mine[id].label };
  },
  unshare: async a => { needRoom(); const ids = a.id ? [a.id] : Object.keys(R.mine); ids.forEach(shareStop); return { stopped: ids }; },
  watch: async a => { needRoom(); const s = findStream(a); watch(s.id, a.on !== false); if (a.on !== false) await until(() => R.remote[s.id], 10000); return Object.assign(s, { receiving: !!R.remote[s.id] }); },
  popout: async a => {
    needRoom(); const s = findStream(a);
    if (!R.watching.has(s.id)) watch(s.id, true);
    if (!(await until(() => R.remote[s.id], 10000))) throw new Error('видео показа не пришло');
    popout(s.id, titleOf(s.peer) + ': ' + s.label);
    return Object.assign(s, { window: !!R.popups[s.id] });
  },
  call: async a => {
    const names = (a.peers || [a.peer]).filter(Boolean);
    if (!names.length) throw new Error('нужны peers');
    const res = await callPeers(names, a.title);
    return Object.assign(roomSummary(), { invited: res });
  },
  answer: async a => {
    const k = a.room || Object.keys(ADHOC).reverse().find(x => INV.some(i => i.id === 'inv-' + x)) || Object.keys(ADHOC).pop();
    if (!k || !ADHOC[k]) throw new Error('нет входящего звонка');
    dropInvite('inv-' + k); ring(false); setVoiceBadge(0);
    await roomJoin(k); await new Promise(r => setTimeout(r, 1200));
    return roomSummary();
  },
  decline: async a => {
    const k = a.room || Object.keys(ADHOC).reverse().find(x => INV.some(i => i.id === 'inv-' + x));
    if (!k) throw new Error('нет входящего звонка');
    dropInvite('inv-' + k); ring(false); setVoiceBadge(0);
    await rsend(k.split(':')[0], { type: 'decline' }, k).catch(() => {});
    return { declined: k };
  },
  volume: async a => { needRoom(); if (!a.peer) throw new Error('нужен peer'); const v = Math.max(0, Math.min(1, +a.value)); setPeerVol(a.peer, v); return { peer: a.peer, volume: v }; },
  ask_screen: async a => { if (!a.peer) throw new Error('нужен peer'); await signal(a.peer, { media: 'room', type: 'ask', room: R.key || ME() + ':ask-' + Date.now().toString(36) }); return { asked: a.peer }; },
};
/** Добавить действие пульта (доска, показ — из своих модулей). */
export function addUi(name: string, f: (a: any) => Promise<any>) { UI[name] = f; }
const uiHooks: ((m: any) => boolean)[] = [];
/** Сообщения узла этой панели (media:'ui'): обработчик возвращает true, если сообщение его. */
export function onUiMsg(f: (m: any) => boolean) { uiHooks.push(f); }
async function onUi(m: any) {
  for (const h of uiHooks) if (h(m)) return;
  if (m.type === 'tab') { setTab(m.tab); try { window.focus(); } catch {} return; }
  if (m.type === 'avatars') { loadAvatars(); return; }
  if (m.type === 'channels') { loadChannels(); return; }
  if (m.type !== 'cmd') return;
  const score = (R.key ? 1000 : 0) + (document.hasFocus() ? 100 : 0) + Math.max(0, 99 - Math.floor((Date.now() - PANEL_T0) / 60000));
  let mine = false;
  try { mine = (await api('POST', 'ui/claim', { id: m.id, panel: PANEL_ID, score })).mine; } catch {}
  if (!mine) return;
  let res: any;
  try {
    const f = UI[m.action];
    if (!f) throw new Error('нет действия «' + m.action + '» (есть: ' + Object.keys(UI).join(', ') + ')');
    res = { ok: true, data: await f(m.args || {}) };
  } catch (e: any) { res = { ok: false, error: e.message }; }
  api('POST', 'ui/result', Object.assign({ id: m.id }, res)).catch(() => {});
}

let started = false;
export function voiceStart() {
  if (started) return;
  started = true;
  loadAvatars(); loadChannels(); loadIce();
  onMedia(m => {
    const kind = m.media || 'voice';
    if (kind === 'room') onRoomQueued(m);
    else if (kind === 'ui') onUi(m);
    else if (kind === 'local') {
      if (m.session === SESSION) return;
      if (m.type === 'takeover') {
        dropInvite('inv-' + m.room); ring(false);
        if (R.key === m.room) { roomLeave(true); toast('Вы в канале с ' + (m.dev || 'другого устройства') + ' — здесь вышли', 'warn'); }
      }
    }
  });
}

// ── приложения (3.5): мост знает, в каком канале этот ПК и кто в нём сейчас (для сеансов приложений),
// и пишет журнал активности: минуты в канале и разговора, показ экрана, пинги, входы соседей ──
export function activity(kind: string, d: Record<string, any> = {}) { api('POST', 'apps/activity/report', { kind, ...d }).catch(() => {}); }
let chanSig = '';
function reportChannel() {
  const members = R.key ? rosterOf(R.key).map(p => p.name).filter(n => n !== ME()) : [];
  const sig = (R.key || '') + '|' + members.join(',');
  if (sig === chanSig) return;
  chanSig = sig;
  api('POST', 'apps/net/channel', { room: R.key || '', title: R.title || '', members }).catch(() => { chanSig = ''; });
}
setInterval(reportChannel, 1500);
setInterval(() => { chanSig = ''; }, 20000); // мост мог перезапуститься — напомнить
let talkMs = 0;
setInterval(() => { if (R.key && SPK.__me) talkMs += 120; }, 120);
setInterval(() => {
  if (!R.key) { talkMs = 0; return; }
  activity('voice.minute', { room: R.key, title: R.title, people: rosterOf(R.key).length, talkSec: Math.min(60, Math.round(talkMs / 1000)) });
  talkMs = 0;
  if (Object.keys(R.mine).length) activity('screen.minute', { room: R.key });
}, 60000);
