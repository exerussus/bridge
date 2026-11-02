/// <reference types="vite/client" />
// Шумоподавление микрофона (3.14). Нейросети работают прямо в панели, в AudioWorklet, без сети:
// RNNoise (Xiph) — лёгкое, полная полоса 48 кГц; GTCRN — сильнее (клавиатура, щелчки, голоса на фоне),
// внутри работает на 16 кГц. Обе — из @sapphi-red/web-noise-suppressor (MIT), wasm и worklet лежат в сборке.
import { RnnoiseWorkletNode, GtcrnWorkletNode, loadRnnoise, loadGtcrn } from '@sapphi-red/web-noise-suppressor';
import rnnoiseWorklet from '@sapphi-red/web-noise-suppressor/rnnoiseWorklet.js?url';
import rnnoiseWasm from '@sapphi-red/web-noise-suppressor/rnnoise.wasm?url';
import rnnoiseSimdWasm from '@sapphi-red/web-noise-suppressor/rnnoise_simd.wasm?url';
import gtcrnWorklet from '@sapphi-red/web-noise-suppressor/gtcrnWorklet.js?url';
import gtcrnWasm from '@sapphi-red/web-noise-suppressor/gtcrn.wasm?url';
import { SET, toast } from '../lib/state.svelte';

export type NsMode = 'off' | 'browser' | 'rnnoise' | 'gtcrn';
/** Варианты для выпадающих списков: [значение, подпись, пояснение]. */
export const NS_MODES: [NsMode, string, string][] = [
  ['off', 'выключено', 'звук микрофона как есть'],
  ['browser', 'обычное', 'встроенное в WebView2: ровный гул и вентилятор'],
  ['rnnoise', 'нейросеть — лёгкое', 'RNNoise: гул, вентилятор, улица; почти не грузит ПК'],
  ['gtcrn', 'нейросеть — сильное', 'GTCRN: ещё клавиатура, щелчки, голоса на фоне; голос чуть глуше'],
];

export function nsMode(): NsMode {
  const m = SET.nsMode;
  return NS_MODES.some(x => x[0] === m) ? m : 'rnnoise';
}
const neural = (m: NsMode) => m === 'rnnoise' || m === 'gtcrn';

// wasm качаем один раз; worklet-модуль добавляется в каждый AudioContext заново
const wasm: Partial<Record<NsMode, Promise<ArrayBuffer>>> = {};
function wasmOf(m: NsMode) {
  return (wasm[m] ??= (m === 'rnnoise' ? loadRnnoise({ url: rnnoiseWasm, simdUrl: rnnoiseSimdWasm }) : loadGtcrn({ url: gtcrnWasm }))
    .catch(e => { delete wasm[m]; throw e; }));
}

async function denoiseNode(ac: AudioContext, m: NsMode): Promise<AudioNode & { destroy?: () => void }> {
  const bin = await wasmOf(m);
  if (m === 'rnnoise') {
    await ac.audioWorklet.addModule(rnnoiseWorklet);
    return new RnnoiseWorkletNode(ac, { maxChannels: 1, wasmBinary: bin });
  }
  await ac.audioWorklet.addModule(gtcrnWorklet);
  return new GtcrnWorkletNode(ac, { maxChannels: 1, wasmBinary: bin });
}

let warned = '';

/** Микрофон с обработкой и громкостью. У результата: _raw — исходный поток (его надо остановить), _ac, _gain, _ns. */
export async function openMic(): Promise<MediaStream> {
  const dev = SET.mic;
  let m = nsMode();
  const ask = (browserNs: boolean) => navigator.mediaDevices.getUserMedia({
    audio: { deviceId: dev ? { exact: dev } : undefined, echoCancellation: SET.ec, noiseSuppression: browserNs, autoGainControl: SET.agc, channelCount: 1 },
    video: false,
  });
  // при нейросети встроенное выключено: два шумодава подряд портят речь
  let raw = await ask(m === 'browser');
  if (!neural(m) && Math.abs(SET.micGain - 1) < 0.01) return raw;
  // RNNoise рассчитан на 48 кГц; если микрофон другой — частоту приведёт сам WebAudio
  const ac = new AudioContext({ sampleRate: 48000, latencyHint: 'interactive' });
  let ns: (AudioNode & { destroy?: () => void }) | null = null;
  if (neural(m)) {
    try { ns = await denoiseNode(ac, m); }
    catch (e: any) {
      // не вышло (старый WebView2, нет WebAssembly…) — откатываемся на встроенное, чтобы не остаться совсем без него
      if (warned !== m) { warned = m; toast('Шумоподавление нейросетью не запустилось: ' + (e?.message || e) + '. Включено обычное.', 'bad'); }
      raw.getTracks().forEach(t => t.stop());
      raw = await ask(true);
      m = 'browser';
    }
  }
  const src = ac.createMediaStreamSource(raw), g = ac.createGain(), dst = ac.createMediaStreamDestination();
  g.gain.value = SET.micGain;
  // строго моно: микрофон часто приходит двумя каналами, а шумодав обрабатывает только первый —
  // без этого второй канал молчит, и голос у других становится на 6 дБ тише (или звучит в одном ухе)
  for (const n of [ns, g, dst]) if (n) { n.channelCount = 1; n.channelCountMode = 'explicit'; n.channelInterpretation = 'speakers'; }
  (ns ? src.connect(ns).connect(g) : src.connect(g)).connect(dst);
  try { await ac.resume(); } catch {}
  const out: any = dst.stream;
  out._raw = raw; out._ac = ac; out._gain = g; out._ns = ns; out._mode = m;
  return out;
}

export function closeMic(s: any) {
  if (!s) return;
  s.getTracks().forEach((t: MediaStreamTrack) => t.stop());
  s._raw?.getTracks().forEach((t: MediaStreamTrack) => t.stop());
  try { s._ns?.disconnect(); s._ns?.destroy?.(); } catch {}
  try { s._ac?.close(); } catch {}
}
