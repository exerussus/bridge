// Звуки панели: сигналы канала, пинг, звонок, свои звуки входа и пинга.
// Всё звучит на выбранном устройстве вывода (как и голоса собеседников). До 3.0 синтезированные сигналы шли
// на устройство «по умолчанию» — если голоса слушали в наушниках, а по умолчанию были колонки, пинг был не слышен.
import { aurl } from './api';
import { SET } from './state.svelte';

let ctx: AudioContext | null = null;
let ctxSink = '';

/** Общий звуковой контекст на выбранном устройстве вывода. */
export function actx(): AudioContext | null {
  try {
    if (!ctx) ctx = new AudioContext({ latencyHint: 'interactive' });
    if (ctx.state === 'suspended') ctx.resume().catch(() => {});
    const want = SET.out || '';
    if (want !== ctxSink && (ctx as any).setSinkId) {
      ctxSink = want;
      (ctx as any).setSinkId(want).catch(() => { ctxSink = ''; });
    }
    return ctx;
  } catch { return null; }
}

// звук разрешён только после действия человека — оживить контекст при первом же щелчке или клавише
for (const ev of ['pointerdown', 'keydown']) addEventListener(ev, () => { if (ctx && ctx.state === 'suspended') ctx.resume().catch(() => {}); }, true);

export function setSink(a: HTMLMediaElement) {
  if (SET.out && (a as any).setSinkId) (a as any).setSinkId(SET.out).catch(() => {});
}

/** Сыграть синтезированный звук: в контексте, а если он «спит» (не было щелчка) — через <audio>. */
function play(build: (c: BaseAudioContext, out: AudioNode, t: number) => number) {
  const c = actx();
  if (c && c.state === 'running') { build(c, c.destination, c.currentTime + 0.01); return; }
  // запасной путь: отрендерить в WAV и сыграть элементом <audio> (его разрешает --autoplay-policy окна)
  try {
    const rate = 44100, len = 2.5;
    const off = new OfflineAudioContext(1, Math.ceil(rate * len), rate);
    const dur = build(off, off.destination, 0.01);
    off.startRendering().then(buf => {
      const n = Math.min(buf.length, Math.ceil(rate * Math.min(len, dur + 0.15)));
      const a = new Audio(URL.createObjectURL(wavOf(buf.getChannelData(0).subarray(0, n), rate)));
      setSink(a);
      a.play().catch(() => {});
      setTimeout(() => URL.revokeObjectURL(a.src), 4000);
    }).catch(() => {});
  } catch {}
}

function wavOf(pcm: Float32Array, rate: number): Blob {
  const b = new ArrayBuffer(44 + pcm.length * 2), v = new DataView(b);
  const w = (o: number, s: string) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
  w(0, 'RIFF'); v.setUint32(4, 36 + pcm.length * 2, true); w(8, 'WAVE'); w(12, 'fmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, rate, true); v.setUint32(28, rate * 2, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true); w(36, 'data'); v.setUint32(40, pcm.length * 2, true);
  let o = 44;
  for (const x of pcm) { const s = Math.max(-1, Math.min(1, x)); v.setInt16(o, s < 0 ? s * 0x8000 : s * 0x7fff, true); o += 2; }
  return new Blob([b], { type: 'audio/wav' });
}

// ── сигналы канала: вход/выход — двухтон вверх/вниз; микрофон и звук — спад/подъём; показ — «искра» ──
const SOUNDS: Record<string, [number, number, number][]> = {
  join: [[587, 0, 0.16], [880, 0.1, 0.26]], leave: [[880, 0, 0.16], [587, 0.1, 0.26]],
  mute: [[520, 0, 0.07], [390, 0.065, 0.12]], unmute: [[390, 0, 0.07], [520, 0.065, 0.12]],
  deaf: [[330, 0, 0.09], [220, 0.085, 0.2]], undeaf: [[220, 0, 0.09], [330, 0.085, 0.2]],
  share: [[660, 0, 0.07], [990, 0.055, 0.07], [1320, 0.11, 0.16]], unshare: [[1320, 0, 0.07], [990, 0.055, 0.07], [660, 0.11, 0.14]],
};

export function sfx(kind: string) {
  if (!SET.sfx || !SOUNDS[kind]) return;
  play((c, dst, t0) => {
    const out = c.createGain(); out.gain.value = 0.5 * SET.sfxVol;
    const lp = c.createBiquadFilter(); lp.type = 'lowpass'; lp.frequency.value = 3200; lp.connect(out); out.connect(dst);
    let end = 0;
    for (const [f, dt, dur] of SOUNDS[kind]) for (const [type, det, vol] of [['sine', 0, 0.16], ['triangle', 7, 0.05]] as [OscillatorType, number, number][]) {
      const o = c.createOscillator(), g = c.createGain(); o.type = type; o.frequency.value = f; o.detune.value = det;
      g.gain.setValueAtTime(0.0001, t0 + dt); g.gain.exponentialRampToValueAtTime(vol, t0 + dt + 0.012); g.gain.exponentialRampToValueAtTime(0.0001, t0 + dt + dur);
      o.connect(g).connect(lp); o.start(t0 + dt); o.stop(t0 + dt + dur + 0.05);
      end = Math.max(end, dt + dur);
    }
    return end;
  });
}

/** Что изменилось у участника — для звука (рация не озвучивается: щёлкало бы на каждом слове). */
export function sfxDiff(prev: any, st: any): string | null {
  if (!prev || !st) return null;
  if (!!prev.deaf !== !!st.deaf) return st.deaf ? 'deaf' : 'undeaf';
  const muted = (x: any) => x.voice && !x.deaf && !x.ptt && !x.mic;
  if (!prev.ptt && !st.ptt && muted(prev) !== muted(st)) return muted(st) ? 'mute' : 'unmute';
  const ns = (x: any) => (x.streams || []).length;
  if (ns(st) > ns(prev)) return 'share';
  if (ns(st) < ns(prev)) return 'unshare';
  return null;
}

// ── пинг ──
export const PING_KINDS: Record<string, { t: string; n: [OscillatorType, number, number, number, number, number][] }> = {
  drop: { t: 'Капля', n: [['sine', 620, 880, 0, 0.07, 0.32], ['sine', 880, 990, 0.11, 0.045, 0.32]] },
  bell: { t: 'Колокольчик', n: [['sine', 1318, 1318, 0, 0.06, 0.9], ['sine', 2637, 2637, 0, 0.02, 0.5], ['sine', 1975, 1975, 0.002, 0.015, 0.6]] },
  tick: { t: 'Щелчок', n: [['triangle', 1400, 900, 0, 0.09, 0.05], ['triangle', 1100, 800, 0.09, 0.06, 0.05]] },
  chime: { t: 'Перелив', n: [['triangle', 660, 660, 0, 0.06, 0.35], ['triangle', 880, 880, 0.12, 0.05, 0.35], ['triangle', 1320, 1320, 0.24, 0.04, 0.5]] },
  soft: { t: 'Тихий', n: [['sine', 520, 600, 0, 0.045, 0.45]] },
};

export function pingSound(kind?: string, vol?: number) {
  const p = PING_KINDS[kind || SET.ping] || PING_KINDS.drop, v = vol ?? SET.pingVol;
  play((c, dst, t) => {
    const lp = c.createBiquadFilter(); lp.type = 'lowpass'; lp.frequency.value = 3000; lp.connect(dst);
    let end = 0;
    for (const [type, f0, f1, dt, g0, dur] of p.n) {
      const o = c.createOscillator(), g = c.createGain(); o.type = type;
      o.frequency.setValueAtTime(f0, t + dt);
      if (f1 !== f0) o.frequency.exponentialRampToValueAtTime(f1, t + dt + Math.min(0.08, dur / 2));
      g.gain.setValueAtTime(0.0001, t + dt); g.gain.exponentialRampToValueAtTime(Math.max(0.0002, g0 * v * 1.4), t + dt + 0.012); g.gain.exponentialRampToValueAtTime(0.0001, t + dt + dur);
      o.connect(g).connect(lp); o.start(t + dt); o.stop(t + dt + dur + 0.05);
      end = Math.max(end, dt + dur);
    }
    return end;
  });
}

let fileA: HTMLAudioElement | null = null;
/** Короткий звук из файла моста (свой пинг, звук входа): не дольше max секунд. */
/** Сыграть файл моста (до max секунд). Возвращает элемент звука (null — не вышло). */
export function playFile(path: string, vol: number, max: number, onFail?: () => void): HTMLAudioElement | null {
  try {
    if (fileA) fileA.pause();
    const a = (fileA = new Audio(aurl(path)));
    a.volume = Math.min(1, vol);
    setSink(a);
    a.play().catch(() => { a.dispatchEvent(new Event('error')); onFail && onFail(); });
    setTimeout(() => { if (fileA === a) a.pause(); }, max * 1000);
    return a;
  } catch { return null; }
}

/** Мой звук пинга: свой файл или встроенный. */
export function playPing(av: { ping?: string }, vol?: number) {
  const v = vol ?? SET.pingVol;
  if (SET.ping === 'custom' && av.ping) { playFile('pingsound?v=' + av.ping, v, 3, () => pingSound('drop', v)); return; }
  pingSound(SET.ping === 'custom' ? 'drop' : SET.ping, v);
}

let ringStop: (() => void) | null = null;
/** Сигнал вызова (пара тонов несколько раз). */
export function ring(on: boolean) {
  if (!on) { ringStop && ringStop(); ringStop = null; return; }
  if (ringStop) return;
  let alive = true;
  ringStop = () => { alive = false; };
  play((c, dst, t0) => {
    let t = t0;
    for (let i = 0; i < 6; i++) {
      const o = c.createOscillator(), g = c.createGain(); o.frequency.value = i % 2 ? 660 : 880;
      g.gain.setValueAtTime(0.0001, t); g.gain.exponentialRampToValueAtTime(0.15, t + 0.02); g.gain.exponentialRampToValueAtTime(0.0001, t + 0.35);
      o.connect(g).connect(dst); o.start(t); o.stop(t + 0.4); t += 0.5;
    }
    return 3;
  });
  setTimeout(() => { if (alive) { ringStop = null; } }, 3500);
}

/** Будильник ассистента: настойчивые короткие гудки. */
export function alarmBeep() {
  play((c, dst, t) => {
    for (let k = 0; k < 4; k++) {
      const o = c.createOscillator(), g = c.createGain(); o.type = 'square'; o.frequency.value = 988;
      g.gain.setValueAtTime(0.0001, t + k * 0.18); g.gain.exponentialRampToValueAtTime(0.08, t + k * 0.18 + 0.01); g.gain.exponentialRampToValueAtTime(0.0001, t + k * 0.18 + 0.12);
      o.connect(g).connect(dst); o.start(t + k * 0.18); o.stop(t + k * 0.18 + 0.14);
    }
    return 0.8;
  });
}

/** Короткий сигнал о сообщении. */
export function msgBeep() {
  play((c, dst, t) => {
    [880, 1175].forEach((f, i) => {
      const o = c.createOscillator(), g = c.createGain(); o.frequency.value = f;
      g.gain.setValueAtTime(0.0001, t + i * 0.12); g.gain.exponentialRampToValueAtTime(0.12, t + i * 0.12 + 0.02); g.gain.exponentialRampToValueAtTime(0.0001, t + i * 0.12 + 0.25);
      o.connect(g).connect(dst); o.start(t + i * 0.12); o.stop(t + i * 0.12 + 0.3);
    });
    return 0.5;
  });
}

/** Проверка вывода: короткий тон. */
export function testTone() {
  play((c, dst, t) => {
    const o = c.createOscillator(), g = c.createGain(); o.frequency.value = 660;
    g.gain.setValueAtTime(0.0001, t); g.gain.exponentialRampToValueAtTime(0.2 * SET.outVol + 0.0002, t + 0.02); g.gain.exponentialRampToValueAtTime(0.0001, t + 0.6);
    o.connect(g).connect(dst); o.start(t); o.stop(t + 0.65);
    return 0.7;
  });
}
