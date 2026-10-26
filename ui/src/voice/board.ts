// @ts-nocheck — холст доски: императивный код, типы здесь только мешают
// ═════════════════════════ Доска канала ═════════════════════════
// Бесконечная общая доска: перо, кисть, маркер, ластик, прямоугольник, стрелка, текст, заметка, картинка, PDF;
// масштаб колесом, перемещение — рукой, средней кнопкой или с зажатым пробелом. Рисуется только то, что попадает в окно
// (сетка ячеек по 512 единиц); картинки грузятся, когда видны и достаточно крупны, и выгружаются, когда их много.
// Слои (3.0): у каждого объекта свой слой; рисуют, стирают и выделяют только в активном слое; порядок слоёв — порядок
// наложения, его можно менять. Фон (цвет и яркость) и слои — общие для всех в канале.
// Доска живёт у владельца канала: изменения уходят ему пачками (частота — «Синхронизация»), обратно — долгим опросом.
import { api, aurl } from '../lib/api';
import { SET, setSet, toast, titleOf, me as ME, openModal, closeModal } from '../lib/state.svelte';
import { esc, size, ago } from '../lib/util';
import { popHtml, closePop, popOpen } from '../lib/pop';
import { bindSets } from '../lib/sets';
import { b64 } from '../lib/chat.svelte';
import { R, SESSION, VV, addUi } from './engine.svelte';
import { setTab } from '../lib/state.svelte';
import HtmlModal from '../modals/HtmlModal.svelte';
import * as DG from './diagram';

const CELL = 512, IMG_POOL = 48, GRID = 24;
const COLORS = ['#E8EAFB', '#38E1F0', '#FF4F64', '#FFB547', '#5DE08E', '#9B7BFF', '#11131F'];
const NOTE_BG = { '#E8EAFB': '#FFE58A', '#38E1F0': '#9FEFF7', '#FF4F64': '#FFB3BD', '#FFB547': '#FFD79A', '#5DE08E': '#B6F1C9', '#9B7BFF': '#D3C6FF', '#11131F': '#E8EAFB' };
const SIZES = [2, 4, 9];
const CUR_COLORS = ['#38E1F0', '#FFB547', '#5DE08E', '#9B7BFF', '#FF8FA0', '#7FD1FF', '#F5E26B'];
const BG_DEFAULT = '#0E0F22';
const BG_PRESETS = [['#0E0F22', 'Ночь'], ['#000000', 'Чёрный'], ['#1C2030', 'Графит'], ['#16302A', 'Школьная'], ['#10223F', 'Синий'], ['#F4F5FA', 'Белый'], ['#F3EEDF', 'Бумага']];
const BASE = 'base';
// шрифты текста и заметок: ключ (в объекте — ff), название, семейство. Все — кириллица и латиница.
export const FONTS = [['', 'Мост', 'Manrope'], ['simple', 'Простой', 'Golos Text'], ['tech', 'Техно', 'Tektur'], ['hand', 'От руки', 'Caveat']];
const FS_STEPS = [8, 10, 12, 14, 16, 18, 20, 24, 28, 32, 40, 48, 56, 64, 72, 96, 128, 160, 200, 256, 320, 400];
const famOf = o => (FONTS.find(f => f[0] === (o.ff || '')) || FONTS[0])[2];
// стиль текста: fb — жирный, fi — курсив, fu — подчёркнутый, fa — выравнивание (left|center|right)
const fontOf = (o, fs) => (o.fi ? 'italic ' : '') + (o.fb ? '700 ' : '500 ') + fs + "px '" + famOf(o) + "', Manrope, sans-serif";
const isText = o => o && (o.t === 'text' || o.t === 'note');
// (3.3) текст есть и у фигур и связей (подпись) — панель текста работает и для них
const hasText = o => o && (isText(o) || o.t === 'shape' || o.t === 'link');
// заливки фигур: нет, тёмные (для тёмной доски), светлые
const FILLS = ['', '#1C2140', '#123A44', '#3A1B2A', '#3A2E14', '#173523', '#2A2152', '#FFFFFF', '#FFE58A', '#9FEFF7', '#FFB3BD', '#B6F1C9', '#D3C6FF'];
const isLightC = c => { const m = /^#([0-9a-f]{6})$/i.exec(c || ''); if (!m) return false; const v = parseInt(m[1], 16); return ((v >> 16) * 0.299 + ((v >> 8) & 255) * 0.587 + (v & 255) * 0.114) > 150; };
const B = {
  room: null, seq: 0, objs: new Map(), cache: new Map(), cells: new Map(), big: new Set(), enabled: false, edit: 'all', owner: '', loaded: false, err: '',
  vis: false, view: { x: -500, y: -320, z: 1 }, W: 0, H: 0, dpr: 1, tool: 'pen', color: COLORS[0], size: 1,
  sel: new Set(), gifVis: new Set(), undo: [], redo: [], imgs: new Map(), curs: new Map(), outq: [], flushT: 0, inflight: false, gen: 0,
  drag: null, follow: '', full: false, mouse: { x: 0, y: 0 }, space: false, dirty: true, drawn: 0, n: 0, lastCur: 0, edit_el: null, miniT: 0, wb: null,
  // общие: фон и слои (снизу вверх)
  bg: { c: '', l: 100 }, layers: [{ id: BASE, name: 'Слой 1' }],
  // свои: активный слой и скрытые слои
  layer: BASE, hidden: new Set(), layersOpen: false,
  // (3.3) диаграммы: какие связи привязаны к объекту, фигура под курсором, направляющие при перетаскивании, временные объекты (предпросмотр)
  att: new Map(), hover: null, guides: [], gaps: [], tmp: null, shapeK: 'rect',
};
const get = id => (B.tmp && B.tmp.get(id)) || B.objs.get(id);
let el: any = {};
const W2S = (x, y) => [(x - B.view.x) * B.view.z, (y - B.view.y) * B.view.z];
const S2W = (x, y) => [x / B.view.z + B.view.x, y / B.view.z + B.view.y];
const newId = () => SESSION + '-' + (++B.n).toString(36) + Date.now().toString(36).slice(-4);
const clean = o => { const c = {}; for (const k in o) if (k[0] !== '_') c[k] = o[k]; return c; };
const canEdit = () => B.enabled && (B.edit !== 'owner' || B.owner === ME());
const snap = v => (SET.bSnap ? Math.round(v / GRID) * GRID : v);
const curColor = n => { let h = 0; for (const ch of n) h = (h * 31 + ch.charCodeAt(0)) >>> 0; return CUR_COLORS[h % CUR_COLORS.length]; };
const imgUrl = id => aurl('board/image?room=' + encodeURIComponent(B.room) + '&id=' + encodeURIComponent(id));
// слои
const layerOf = o => o.L || BASE;
const layerIdx = id => { const i = B.layers.findIndex(l => l.id === id); return i < 0 ? 0 : i; };
const layerShown = id => !B.hidden.has(id);
const inActive = o => layerOf(o) === B.layer;
const activeLayer = () => B.layers.find(l => l.id === B.layer) || B.layers[0];
function fixActive() { if (!B.layers.some(l => l.id === B.layer)) B.layer = (B.layers[B.layers.length - 1] || { id: BASE }).id; }
// фон: цвет с яркостью
function bgColor() {
  const c = /^#[0-9a-f]{6}$/i.test(B.bg.c || '') ? B.bg.c : BG_DEFAULT, k = Math.max(0.2, Math.min(2, (+B.bg.l || 100) / 100));
  const ch = i => { const v = parseInt(c.slice(1 + i * 2, 3 + i * 2), 16); const r = k <= 1 ? v * k : v + (255 - v) * (k - 1); return Math.round(Math.max(0, Math.min(255, r))); };
  return 'rgb(' + ch(0) + ',' + ch(1) + ',' + ch(2) + ')';
}
function bgLight() { const m = /rgb\((\d+),(\d+),(\d+)\)/.exec(bgColor()); if (!m) return false; return (+m[1] * 0.299 + +m[2] * 0.587 + +m[3] * 0.114) > 150; }

// ───────────── разметка ─────────────
const SVG = p => '<svg class="gi" viewBox="0 0 24 24">' + p + '</svg>';
const TOOLS = [
  ['select', 'Выбор (V)', '<path d="M5 3l14 8-6 2-2 6z"/>'],
  ['hand', 'Рука — двигать доску (H, пробел, средняя кнопка)', '<path d="M8 12V6a1.5 1.5 0 0 1 3 0v5M11 11V4.5a1.5 1.5 0 0 1 3 0V11M14 11V6a1.5 1.5 0 0 1 3 0v7a7 7 0 0 1-12.5 4.3L3 15a1.5 1.5 0 0 1 2.4-1.8L8 15"/>'],
  ['pen', 'Перо (P)', '<path d="M4 20l4-1 10-10-3-3L5 16z"/><path d="M14 6l3 3"/>'],
  ['brush', 'Кисть (B) — радиус в пикселях задаётся ползунком', '<path d="M14 4l6 6-7 7-4-4zM9 13l-3 1c-2 .7-2 3.5-3 6 2.5-1 5.3-1 6-3l1-3"/>'],
  ['marker', 'Маркер (M)', '<path d="M9 15l-4 4h5l2-2M9 15l7-7 3 3-7 7z"/>'],
  ['eraser', 'Ластик (E) — стирает только в активном слое', '<path d="M8 20h12M5 15l9-9 5 5-8 8H8z"/>'],
  ['-'],
  ['rect', 'Прямоугольник (R)', '<rect x="4" y="6" width="16" height="12" rx="1.5"/>'],
  ['arrow', 'Стрелка (A)', '<path d="M5 19L19 5M11 5h8v8"/>'],
  ['shape', 'Фигура (S) — щёлкните, чтобы выбрать фигуру; тяните по доске или щёлкните', ''],
  ['link', 'Связь (L) — тяните от фигуры к фигуре; связь едет за фигурами', '<circle cx="5.5" cy="18.5" r="2.5"/><circle cx="18.5" cy="5.5" r="2.5"/><path d="M7.5 16.5l9-9"/>'],
  ['text', 'Текст (T)', '<path d="M5 6V4h14v2M12 4v16M9 20h6"/>'],
  ['note', 'Заметка (N)', '<path d="M4 4h16v11l-5 5H4z"/><path d="M15 20v-5h5"/>'],
  ['image', 'Картинка или PDF (I) — или вставьте Ctrl+V, или перетащите файл', '<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 8"/>'],
];
const IC_LAYERS = '<path d="M12 3l9 5-9 5-9-5z"/><path d="M3 13l9 5 9-5"/>';
function mount(host) {
  if (!host || host.dataset.on) return;
  host.dataset.on = '1';
  host.innerHTML = '<div class="bd" tabindex="-1">' +
    '<div class="bdtop"><b class="bdtitle"></b><span class="small muted">доска · развёрнута</span><div class="grow"></div><button class="b sm" data-bd="full">' + SVG('<path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5"/>') + ' Свернуть · Esc</button></div>' +
    '<canvas class="bdcv"></canvas><div class="bdgifs"></div><textarea class="btext hide" spellcheck="false"></textarea>' +
    '<div class="fl bdtools" role="toolbar" aria-label="Инструменты доски">' + TOOLS.map(t => (t[0] === '-' ? '<i class="bdsep"></i>' : '<button class="tool" data-tool="' + t[0] + '" title="' + t[1] + '" aria-label="' + t[1] + '">' + SVG(t[2]) + '</button>')).join('') +
      '<i class="bdsep"></i><button class="tool" data-bd="layers" title="Слои (Shift+L)">' + SVG(IC_LAYERS) + '</button></div>' +
    '<div class="fl bdprops">' + COLORS.map(c => '<button class="sw" data-color="' + c + '" style="background:' + c + '" title="' + c + '"></button>').join('') + '<i class="bdsep v"></i><span class="szs">' + SIZES.map((s, i) => '<button class="szb" data-size="' + i + '" title="Толщина"><i style="width:' + (5 + i * 5) + 'px;height:' + (5 + i * 5) + 'px"></i></button>').join('') + '</span>' +
      '<label class="brr hide" title="Радиус кисти в пикселях экрана"><input type="range" min="1" max="120" step="1"><b>12 px</b></label><i class="bdsep v"></i><span class="bdlay small" title="Активный слой — рисуете и стираете здесь"></span></div>' +
    '<div class="fl bdtxt hide" role="toolbar" aria-label="Текст">' + FONTS.map(f => '<button class="b sm ff" data-ff="' + f[0] + '" style="font-family:\'' + f[2] + '\',Manrope,sans-serif" title="Шрифт «' + f[1] + '»">' + f[1] + '</button>').join('') +
      '<i class="bdsep v"></i><button class="tool" data-tx="dn" title="Меньше">' + SVG('<path d="M6 12h12"/>') + '</button><input class="txtfs" type="number" min="4" max="800" title="Размер текста"><button class="tool" data-tx="up" title="Больше">' + SVG('<path d="M12 6v12M6 12h12"/>') + '</button>' +
      '<i class="bdsep v"></i><button class="tool" data-tx="fb" title="Жирный (Ctrl+B)"><b style="font-size:15px">Ж</b></button><button class="tool" data-tx="fi" title="Курсив (Ctrl+I)"><i style="font:italic 600 15px Georgia,serif">К</i></button><button class="tool" data-tx="fu" title="Подчёркнутый (Ctrl+U)"><u style="font-size:15px;font-weight:600">Ч</u></button>' +
      '<i class="bdsep v"></i><button class="tool" data-tx="left" title="По левому краю">' + SVG('<path d="M4 6h16M4 10h10M4 14h16M4 18h10"/>') + '</button><button class="tool" data-tx="center" title="По центру">' + SVG('<path d="M4 6h16M7 10h10M4 14h16M7 18h10"/>') + '</button><button class="tool" data-tx="right" title="По правому краю">' + SVG('<path d="M4 6h16M10 10h10M4 14h16M10 18h10"/>') + '</button>' +
      '<span class="txva"><i class="bdsep v"></i><button class="tool" data-tx="vtop" title="Текст в фигуре — сверху">' + SVG('<path d="M4 4h16M8 9h8M8 13h8"/>') + '</button><button class="tool" data-tx="vmid" title="Текст в фигуре — по середине">' + SVG('<path d="M8 10h8M8 14h8M4 4h16M4 20h16"/>') + '</button><button class="tool" data-tx="vbot" title="Текст в фигуре — снизу">' + SVG('<path d="M4 20h16M8 11h8M8 15h8"/>') + '</button></span></div>' +
    '<div class="fl bddg hide" role="toolbar" aria-label="Фигуры и связи"></div>' +
    '<div class="fl bdright"><button class="tool" data-bd="undo" title="Отменить (Ctrl+Z)">' + SVG('<path d="M9 14L4 9l5-5"/><path d="M4 9h10a6 6 0 0 1 0 12h-3"/>') + '</button><button class="tool" data-bd="redo" title="Вернуть (Ctrl+Shift+Z)">' + SVG('<path d="M15 14l5-5-5-5"/><path d="M20 9H10a6 6 0 0 0 0 12h3"/>') + '</button><i class="bdsep v"></i>' +
      '<button class="b sm" data-bd="export">' + SVG('<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>') + ' Экспорт</button><button class="b sm" data-bd="stands">' + SVG('<path d="M4 7h16M4 12h16M4 17h10"/>') + ' Стенды</button><i class="bdsep v"></i>' +
      '<button class="tool" data-bd="cfg" title="Настройки доски">' + SVG('<circle cx="12" cy="12" r="3"/><path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1"/>') + '</button><button class="tool" data-bd="full" title="Развернуть доску на всё окно">' + SVG('<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>') + '</button></div>' +
    '<div class="fl bdzoom"><button class="tool" data-bd="zout" title="Уменьшить (−)">' + SVG('<path d="M6 12h12"/>') + '</button><span class="bdz">100%</span><button class="tool" data-bd="zin" title="Увеличить (+)">' + SVG('<path d="M12 6v12M6 12h12"/>') + '</button><button class="b sm" data-bd="fit" title="Показать всё (Ctrl+0)">Вписать</button></div>' +
    '<div class="fl bdstat"><i class="bddot"></i><span class="bdst"></span><button class="b sm" data-bd="off">Выключить</button><span class="muted bdxy"></span></div>' +
    '<canvas class="fl bdmini" width="200" height="124" title="Вся доска — щёлкните, чтобы перейти"></canvas>' +
    '<div class="bdlayers hide" role="complementary" aria-label="Слои доски"></div>' +
    '<div class="bdcfg hide" role="complementary" aria-label="Настройки доски"><div class="row"><b>Настройки доски</b><div class="grow"></div><button class="tool" data-bd="cfg" title="Закрыть">' + SVG('<path d="M6 6l12 12M18 6L6 18"/>') + '</button></div>' +
      '<div class="plbl">Фон доски <span class="muted">· общий для всех</span></div><div class="bdbg"></div>' +
      '<div class="ptog"><span>Яркость</span><input type="range" class="bdbright" min="30" max="170" step="5" style="flex:1"><em class="bdbrightv"></em></div>' +
      '<div class="psep"></div><div class="plbl">Синхронизация</div><div class="seg" data-set="bSync"><button data-v="16">сразу</button><button data-v="50">20/с</button><button data-v="100">10/с</button><button data-v="250">4/с</button></div><div class="small muted">Как часто ваши штрихи уходят остальным. Реже — меньше трафика по слабой сети.</div>' +
      '<label class="ptog"><input type="checkbox" data-set="bCursors"><span>Курсоры других</span></label>' +
      '<div class="ptog"><span>Следовать за видом</span><select class="bdfollow"></select></div>' +
      '<div class="psep"></div><div class="plbl">Холст</div><div class="seg" data-set="bGrid"><button data-v="dots">точки</button><button data-v="grid">клетка</button><button data-v="none">пусто</button></div>' +
      '<label class="ptog"><input type="checkbox" data-set="bSnap"><span>Привязка к сетке</span></label><label class="ptog"><input type="checkbox" data-set="bSmooth"><span>Сглаживать штрихи</span></label>' +
      '<div class="psep"></div><div class="plbl">Картинки</div><div class="seg" data-set="bImg"><button data-v="0">как есть</button><button data-v="2560">до 2560 px</button><button data-v="1280">до 1280 px</button></div>' +
      '<div class="psep"></div><div class="plbl">Кто правит</div><div class="seg bdeditseg"><button data-em="all">все в канале</button><button data-em="owner">только владелец</button></div><div class="small muted bdeditnote"></div>' +
      '<div class="psep"></div><div class="plbl">Сейчас</div><div class="bdstats small"></div></div>' +
    '<div class="bdoff hide"><svg class="gi" viewBox="0 0 24 24" style="width:40px;height:40px;stroke:var(--cyan)"><path d="M4 20l4-1 10-10-3-3L5 16z"/><path d="M14 6l3 3"/></svg><h3>Доска выключена</h3><div class="small muted bdofftext">Общая доска для рисования, картинок, PDF и заметок. Включить может любой участник канала — увидят и смогут править все, кто здесь.</div><div class="row" style="justify-content:center"><button class="b pri" data-bd="on">Включить доску</button><button class="b" data-bd="stands">Открыть стенд…</button></div></div>' +
    '</div>';
  const q = s => host.querySelector(s);
  el = { host, gifs: q('.bdgifs'), brr: q('.brr input'), brrv: q('.brr b'), bd: q('.bd'), cv: q('.bdcv'), mini: q('.bdmini'), text: q('.btext'), z: q('.bdz'), st: q('.bdst'), xy: q('.bdxy'), dot: q('.bddot'), off: q('.bdoff'), cfg: q('.bdcfg'), title: q('.bdtitle'), stats: q('.bdstats'), follow: q('.bdfollow'), layers: q('.bdlayers'), lay: q('.bdlay'), bgbox: q('.bdbg'), bright: q('.bdbright'), brightv: q('.bdbrightv'), txt: q('.bdtxt'), txtfs: q('.txtfs'), dg: q('.bddg') };
  B.shapeK = DG.SHAPE_NAME[SET.bShape] ? SET.bShape : 'rect';
  el.dg.addEventListener('pointerdown', e => { if (!e.target.closest('input')) e.preventDefault(); });
  el.dg.addEventListener('click', dgClick);
  el.ctx = el.cv.getContext('2d');
  bindSets(el.cfg, () => dirty());
  new ResizeObserver(() => resize()).observe(el.bd);
  const cv = el.cv;
  cv.addEventListener('pointerdown', down);
  cv.addEventListener('pointermove', move);
  cv.addEventListener('pointerup', up);
  cv.addEventListener('pointercancel', up);
  cv.addEventListener('dblclick', dbl);
  cv.addEventListener('wheel', wheel, { passive: false });
  cv.addEventListener('pointerleave', () => { B.mouse.in = false; });
  el.bd.addEventListener('dragover', e => { const t = e.dataTransfer.types; if (t.includes('Files') || t.includes('text/uri-list') || t.includes('text/html')) e.preventDefault(); });
  el.bd.addEventListener('drop', async e => {
    e.preventDefault();
    const r = el.cv.getBoundingClientRect(); const [x, y] = S2W(e.clientX - r.left, e.clientY - r.top);
    const files = [...e.dataTransfer.files];
    if (!files.length) { // картинка или гифка, перетащенная из чата или стикеров
      const url = (e.dataTransfer.getData('text/uri-list') || '').split('\n').find(u => u && !u.startsWith('#')) || (/src="([^"]+)"/.exec(e.dataTransfer.getData('text/html') || '') || [])[1];
      if (url) {
        try {
          const u = new URL(url, location.href);
          if (u.origin !== location.origin) throw new Error('картинку с чужого сайта сначала сохраните');
          const resp = await fetch(u); if (!resp.ok) throw new Error('не скачалась');
          const blob = await resp.blob();
          files.push(new File([blob], 'из-чата.' + (blob.type.split('/')[1] || 'png').replace('jpeg', 'jpg'), { type: blob.type }));
        } catch (err) { toast('Картинка: ' + err.message, 'bad'); return; }
      }
    }
    addFiles(files, x, y);
  });
  el.mini.addEventListener('pointerdown', miniNav);
  el.mini.addEventListener('pointermove', e => { if (e.buttons & 1) miniNav(e); });
  el.follow.onchange = () => { B.follow = el.follow.value; };
  el.brr.value = SET.bBrush || 12; el.brrv.textContent = el.brr.value + ' px';
  el.brr.oninput = () => { setSet('bBrush', +el.brr.value); el.brrv.textContent = el.brr.value + ' px'; dirty(); };
  el.bright.oninput = () => { el.brightv.textContent = el.bright.value + '%'; B.bg = { ...B.bg, l: +el.bright.value }; dirty(); };
  el.bright.onchange = () => setBg({ l: +el.bright.value });
  host.addEventListener('click', click);
  el.text.addEventListener('input', textInput);
  el.text.addEventListener('blur', textDone);
  el.text.addEventListener('keydown', e => {
    if (e.key === 'Escape') { e.preventDefault(); el.text.blur(); }
    if (e.key === 'Tab') { const o = B.objs.get(B.edit_el); if (o && o.t === 'shape') { e.preventDefault(); el.text.blur(); quickAdd(B.objs.get(o.id), B.lastSide || 'r'); } }
    const k = { b: 'fb', и: 'fb', i: 'fi', ш: 'fi', u: 'fu', г: 'fu' }[e.key.toLowerCase()];
    if ((e.ctrlKey || e.metaKey) && k) { e.preventDefault(); txtSet(k, null); }
    e.stopPropagation();
  });
  el.txt.addEventListener('pointerdown', e => { if (!e.target.closest('input')) e.preventDefault(); }); // не терять фокус поля текста
  el.txt.addEventListener('click', txtClick);
  el.txtfs.addEventListener('change', () => { const v = Math.max(4, Math.min(800, Math.round(+el.txtfs.value || 0))); if (v) txtSet('fs', v); });
  el.txtfs.addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); el.txtfs.blur(); } e.stopPropagation(); });
  paintTools();
  requestAnimationFrame(frame);
}

// ───────────── сеть ─────────────
async function load() {
  const room = B.room, gen = B.gen;
  try {
    const r = await api('GET', 'board/state?room=' + encodeURIComponent(room));
    if (gen !== B.gen) return false;
    B.objs.clear(); B.cache.clear(); B.cells.clear(); B.big.clear(); B.sel.clear(); B.att.clear(); B.wb = null;
    for (const o of r.objs || []) setObj(o, true);
    B.seq = r.seq || 0; B.enabled = !!r.enabled; B.edit = r.edit || 'all'; B.owner = r.owner || ''; B.loaded = true; B.err = '';
    applyMeta(r);
    if (!B.fitted && B.objs.size && B.vis && B.W > 0) { B.fitted = true; fit(); }
    ui(); dirty();
    return true;
  } catch (e) { if (gen === B.gen) { B.err = e.message; ui(); } return false; }
}
function applyMeta(r) {
  if (r.bg && typeof r.bg === 'object') B.bg = { c: r.bg.c || '', l: +r.bg.l || 100 };
  if (Array.isArray(r.layers) && r.layers.length) B.layers = r.layers.filter(l => l && l.id).map(l => ({ id: String(l.id), name: String(l.name || l.id) }));
  if (!B.layers.length) B.layers = [{ id: BASE, name: 'Слой 1' }];
  fixActive(); paintLayers(); paintBg();
}
async function pollLoop(gen) {
  while (gen === B.gen) {
    if (!B.loaded) { if (!(await load())) { await sleep(4000); continue; } }
    try {
      const r = await api('GET', 'board/poll?room=' + encodeURIComponent(B.room) + '&after=' + B.seq + '&timeout=25');
      if (gen !== B.gen) return;
      if (r.resync) { B.loaded = false; continue; }
      for (const ev of r.events || []) { if (ev.seq <= B.seq) continue; B.seq = ev.seq; if (ev.cs !== SESSION) applyOp(ev.op, ev.by); }
    } catch { await sleep(3000); }
  }
}
const sleep = ms => new Promise(r => setTimeout(r, ms));
function send(op) {
  if (!B.room) return;
  if (op.t === 'put') { const i = B.outq.findIndex(x => x.t === 'put' && x.o.id === op.o.id); if (i >= 0) { B.outq[i] = op; return sched(); } }
  if (op.t === 'cur' || op.t === 'bg' || op.t === 'layers') { const i = B.outq.findIndex(x => x.t === op.t); if (i >= 0) { B.outq[i] = op; return sched(); } }
  B.outq.push(op); sched();
}
function sched() { if (B.flushT) return; B.flushT = setTimeout(flush, Math.max(16, +SET.bSync || 50)); }
let errT = 0;
async function flush() {
  B.flushT = 0;
  if (B.inflight || !B.outq.length) { if (B.outq.length) sched(); return; }
  const ops = B.outq.splice(0, 300).map(op => (op.t === 'put' ? { t: 'put', o: clean(op.o) } : op));
  B.inflight = true;
  try { await api('POST', 'board/op', { room: B.room, cs: SESSION, ops }); }
  catch (e) { if (Date.now() - errT > 5000) { errT = Date.now(); toast('Доска: ' + e.message, 'bad'); } if (/выключена|только владелец/.test(e.message)) { B.loaded = false; load(); } }
  B.inflight = false;
  if (B.outq.length) sched();
}

// ───────────── объекты и индекс ─────────────
function setObj(o, quiet) {
  const old = B.objs.get(o.id); if (old) { unindex(o.id); if (old.t === 'link') detach(old); }
  B.objs.set(o.id, o); B.cache.delete(o.id); if (o.t === 'link') attach(o); index(o); B.wb = null;
  relink(o.id);
  if (!quiet) dirty();
}
function delObj(id) { const o = B.objs.get(id); if (!o) return; unindex(id); if (o.t === 'link') detach(o); B.objs.delete(id); B.cache.delete(id); B.sel.delete(id); B.wb = null; relink(id); dirty(); }
// связи: кто к кому привязан; фигура сдвинулась — связи пересчитать
function ends(l) { return [l.a && l.a.id, l.b && l.b.id].filter(Boolean); }
function attach(l) { for (const id of ends(l)) { let s = B.att.get(id); if (!s) B.att.set(id, (s = new Set())); s.add(l.id); } }
function detach(l) { for (const id of ends(l)) { const s = B.att.get(id); if (s) { s.delete(l.id); if (!s.size) B.att.delete(id); } } }
function relink(id) { const s = B.att.get(id); if (!s) return; for (const lid of s) { const l = B.objs.get(lid); if (!l || lid === id) continue; unindex(lid); B.cache.delete(lid); index(l); } }
/** Связи, привязанные к этим объектам (для удаления вместе с фигурами). */
function linksOf(ids) { const out = new Set(); for (const id of ids) for (const lid of B.att.get(id) || []) if (!ids.has(lid)) out.add(lid); return out; }
function applyOp(op, by) {
  switch (op.t) {
    case 'put': setObj(op.o); break;
    case 'del': for (const id of op.ids || []) delObj(id); break;
    case 'clear': for (const id of [...B.objs.keys()]) delObj(id); break;
    case 'load':
      for (const id of [...B.objs.keys()]) delObj(id);
      for (const o of op.objs || []) setObj(o, true);
      if (op.bg || op.layers) applyMeta(op);
      B.wb = null; dirty(); break;
    case 'enable': B.enabled = !!op.on; ui(); break;
    case 'edit': B.edit = op.mode === 'owner' ? 'owner' : 'all'; ui(); break;
    case 'bg': B.bg = { c: op.c || '', l: +op.l || 100 }; paintBg(); dirty(); break;
    case 'layers': applyMeta({ layers: op.list }); dirty(); break;
    case 'cur':
      if (by && by !== ME()) {
        if (op.gone) B.curs.delete(by);
        else {
          B.curs.set(by, { x: op.x, y: op.y, vx: op.vx, vy: op.vy, vz: op.vz, t: Date.now() });
          if (B.follow === by && op.vz) { B.view.z = op.vz; B.view.x = op.vx - B.W / 2 / op.vz; B.view.y = op.vy - B.H / 2 / op.vz; }
        }
        dirty();
      }
      break;
  }
}
function box(o) {
  let c = B.cache.get(o.id); if (c && c.box) return c.box;
  c = c || {}; let b;
  switch (o.t) {
    case 'path': {
      const p = o.pts || []; let x0 = 1e9, y0 = 1e9, x1 = -1e9, y1 = -1e9;
      for (let i = 0; i < p.length; i += 2) { x0 = Math.min(x0, p[i]); x1 = Math.max(x1, p[i]); y0 = Math.min(y0, p[i + 1]); y1 = Math.max(y1, p[i + 1]); }
      const w = (o.w || 2) / 2 + 1; b = p.length ? [o.x + x0 - w, o.y + y0 - w, o.x + x1 + w, o.y + y1 + w] : [o.x, o.y, o.x, o.y]; break;
    }
    case 'arrow': { const w = (o.sw || 3) * 4; b = [Math.min(o.x, o.x2) - w, Math.min(o.y, o.y2) - w, Math.max(o.x, o.x2) + w, Math.max(o.y, o.y2) + w]; break; }
    case 'text': {
      const lines = String(o.text || ' ').split('\n'), fs = o.fs || 20; const ctx = el.ctx || document.createElement('canvas').getContext('2d'); ctx.font = fontOf(o, fs);
      const w = Math.max(fs, ...lines.map(l => ctx.measureText(l || ' ').width)); c.lines = lines; b = [o.x, o.y, o.x + w, o.y + lines.length * fs * 1.3]; break;
    }
    case 'note': {
      const fs = o.fs || 16, w = o.w || 220, ctx = el.ctx || document.createElement('canvas').getContext('2d'); ctx.font = fontOf(o, fs);
      c.lines = wrap(ctx, String(o.text || ''), w - 28); const h = Math.max(o.h || 0, 28 + c.lines.length * fs * 1.35); c.h = h; b = [o.x, o.y, o.x + w, o.y + h]; break;
    }
    case 'shape': {
      b = [o.x, o.y, o.x + (o.w || 0), o.y + (o.h || 0)];
      if (o.text) { const fs = o.fs || 18, ctx = mctx(); ctx.font = fontOf(o, fs); const tb = DG.textBox(o); c.lines = o.k === 'lane' ? String(o.text).split('\n') : wrap(ctx, String(o.text), Math.max(fs, tb[2])); }
      break;
    }
    case 'link': {
      const g = DG.linkGeom(o, get, x => box(x)); c.g = g; const w = Math.max(10, (o.sw || 2) * 5);
      let x0 = 1e9, y0 = 1e9, x1 = -1e9, y1 = -1e9; for (const [x, y] of g.poly) { x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y); }
      b = [x0 - w, y0 - w, x1 + w, y1 + w];
      if (o.text) { const fs = o.fs || 15, ctx = mctx(); ctx.font = fontOf(o, fs); c.lines = String(o.text).split('\n'); const lw = Math.max(...c.lines.map(l => ctx.measureText(l).width)) + fs, lh = c.lines.length * fs * 1.3 + fs * 0.5; const [px, py] = DG.along(g.poly, o.lt ?? 0.5); c.lb = [px - lw / 2, py - lh / 2, px + lw / 2, py + lh / 2]; b = [Math.min(b[0], c.lb[0]), Math.min(b[1], c.lb[1]), Math.max(b[2], c.lb[2]), Math.max(b[3], c.lb[3])]; }
      break;
    }
    default: { const x0 = Math.min(o.x, o.x + (o.w || 0)), y0 = Math.min(o.y, o.y + (o.h || 0)); b = [x0, y0, x0 + Math.abs(o.w || 0), y0 + Math.abs(o.h || 0)]; }
  }
  c.box = b; B.cache.set(o.id, c); return b;
}
let mc = null; const mctx = () => el.ctx || mc || (mc = document.createElement('canvas').getContext('2d'));
function wrap(ctx, text, w) { const out = []; for (const para of text.split('\n')) { let line = ''; for (const word of para.split(' ')) { const t = line ? line + ' ' + word : word; if (ctx.measureText(t).width > w && line) { out.push(line); line = word; } else line = t; } out.push(line); } return out; }
function cellsOf(b) { return [Math.floor(b[0] / CELL), Math.floor(b[1] / CELL), Math.floor(b[2] / CELL), Math.floor(b[3] / CELL)]; }
function index(o) {
  const [i0, j0, i1, j1] = cellsOf(box(o)); const c = B.cache.get(o.id);
  if ((i1 - i0 + 1) * (j1 - j0 + 1) > 256) { B.big.add(o.id); c.cells = null; return; }
  c.cells = [];
  for (let i = i0; i <= i1; i++) for (let j = j0; j <= j1; j++) { const k = i + ',' + j; let s = B.cells.get(k); if (!s) B.cells.set(k, (s = new Set())); s.add(o.id); c.cells.push(k); }
}
function unindex(id) { const c = B.cache.get(id); B.big.delete(id); if (!c || !c.cells) return; for (const k of c.cells) { const s = B.cells.get(k); if (s) { s.delete(id); if (!s.size) B.cells.delete(k); } } }
/** Объекты в рамке, по порядку наложения: слой (снизу вверх), потом время. Скрытые слои не входят. */
function query(x0, y0, x1, y1, all) {
  const out = new Set(B.big), [i0, j0, i1, j1] = cellsOf([x0, y0, x1, y1]);
  if ((i1 - i0 + 1) * (j1 - j0 + 1) > B.cells.size) { for (const s of B.cells.values()) for (const id of s) out.add(id); }
  else for (let i = i0; i <= i1; i++) for (let j = j0; j <= j1; j++) { const s = B.cells.get(i + ',' + j); if (s) for (const id of s) out.add(id); }
  const res = [];
  for (const id of out) { const o = B.objs.get(id); if (!o) continue; if (!all && !layerShown(layerOf(o))) continue; const b = box(o); if (b[2] >= x0 && b[0] <= x1 && b[3] >= y0 && b[1] <= y1) res.push(o); }
  return res.sort(order);
}
// контейнеры (рамки, дорожки) — под остальным в своём слое
const order = (a, b) => layerIdx(layerOf(a)) - layerIdx(layerOf(b)) || (DG.isContainer(a) ? 0 : 1) - (DG.isContainer(b) ? 0 : 1) || (a.z || 0) - (b.z || 0);
function worldBounds() {
  if (B.wb) return B.wb; let b = null;
  for (const o of B.objs.values()) { const x = box(o); b = b ? [Math.min(b[0], x[0]), Math.min(b[1], x[1]), Math.max(b[2], x[2]), Math.max(b[3], x[3])] : x.slice(); }
  return (B.wb = b);
}

// ───────────── изменения с историей ─────────────
function change(before, after) {
  const ops = [];
  for (const [id, o] of after) { if (o) { setObj(o); ops.push({ t: 'put', o }); } else { delObj(id); ops.push({ t: 'del', ids: [id] }); } }
  ops.forEach(send);
  B.undo.push({ before, after }); if (B.undo.length > 200) B.undo.shift(); B.redo = [];
}
function hist(before, after) { B.undo.push({ before, after }); if (B.undo.length > 200) B.undo.shift(); B.redo = []; }
function replay(states) { const dels = []; for (const [id, o] of states) { if (o) { setObj(o); send({ t: 'put', o }); } else { delObj(id); dels.push(id); } } if (dels.length) send({ t: 'del', ids: dels }); }
function undo() { if (!canEdit()) return; const h = B.undo.pop(); if (!h) return; replay(h.before); B.redo.push(h); }
function redo() { if (!canEdit()) return; const h = B.redo.pop(); if (!h) return; replay(h.after); B.undo.push(h); }
const copy = o => (o ? JSON.parse(JSON.stringify(clean(o))) : null);

// ───────────── отрисовка ─────────────
function dirty() { B.dirty = true; }
function resize() {
  if (!el.bd) return;
  const r = el.bd.getBoundingClientRect(), top = B.full ? 52 : 0; B.dpr = devicePixelRatio || 1;
  const cx = B.W ? B.view.x + B.W / 2 / B.view.z : null, cy = B.H ? B.view.y + B.H / 2 / B.view.z : null;
  B.W = r.width; B.H = r.height - top;
  el.cv.style.top = top + 'px'; el.cv.width = Math.max(1, B.W * B.dpr); el.cv.height = Math.max(1, B.H * B.dpr); el.cv.style.width = B.W + 'px'; el.cv.style.height = B.H + 'px';
  if (cx !== null) { B.view.x = cx - B.W / 2 / B.view.z; B.view.y = cy - B.H / 2 / B.view.z; }
  dirty();
}
function frame() {
  requestAnimationFrame(frame);
  if (!B.vis || !el.ctx) return;
  // курсоры гаснут сами
  for (const [n, c] of B.curs) if (Date.now() - c.t > 15000) { B.curs.delete(n); B.dirty = true; }
  if (!B.dirty) return;
  B.dirty = false;
  render();
  if (Date.now() - B.miniT > 250) { B.miniT = Date.now(); mini(); }
}
function render() {
  const ctx = el.ctx, z = B.view.z, d = B.dpr;
  ctx.setTransform(1, 0, 0, 1, 0, 0); ctx.fillStyle = bgColor(); ctx.fillRect(0, 0, el.cv.width, el.cv.height);
  grid(ctx);
  ctx.setTransform(d * z, 0, 0, d * z, -B.view.x * z * d, -B.view.y * z * d);
  const [x0, y0] = S2W(0, 0), [x1, y1] = S2W(B.W, B.H);
  const list = query(x0, y0, x1, y1); B.drawn = list.length; B.gifVis = new Set();
  const seen = new Set();
  for (const o of list) drawObj(ctx, o, z, seen);
  for (const [id, im] of B.imgs) if (seen.has(id)) im.last = Date.now();
  gifs();
  { const sig = B.tool + '|' + (B.edit_el || '') + '|' + [...B.sel].join(','); if (sig !== B.txtSig) { B.txtSig = sig; paintTxt(); } }
  { const sig = dgSig(); if (sig !== B.dgSig) { B.dgSig = sig; paintDg(); } }
  // выделение
  ctx.setTransform(d, 0, 0, d, 0, 0);
  const bgc = bgColor();
  for (const id of B.sel) {
    const o = B.objs.get(id); if (!o) continue;
    if (o.t === 'link') { const c = B.cache.get(o.id); if (!c || !c.g) continue; ctx.strokeStyle = 'rgba(56,225,240,.55)'; ctx.lineWidth = Math.max(4, (o.sw || 2) * z + 4); ctx.lineCap = 'round'; ctx.lineJoin = 'round'; ctx.globalAlpha = 0.35; ctx.beginPath(); c.g.poly.forEach(([x, y], i) => { const [qx, qy] = W2S(x, y); i ? ctx.lineTo(qx, qy) : ctx.moveTo(qx, qy); }); ctx.stroke(); ctx.globalAlpha = 1;
      if (B.sel.size === 1) for (const [e, att] of [[c.g.a, c.g.attA], [c.g.b, c.g.attB]]) { const [qx, qy] = W2S(e[0], e[1]); ctx.beginPath(); ctx.arc(qx, qy, 6, 0, Math.PI * 2); ctx.fillStyle = att ? '#38E1F0' : bgc; ctx.fill(); ctx.strokeStyle = '#38E1F0'; ctx.lineWidth = 2; ctx.stroke(); }
      continue; }
    const b = box(o); const [sx, sy] = W2S(b[0], b[1]), [ex, ey] = W2S(b[2], b[3]);
    ctx.strokeStyle = '#38E1F0'; ctx.setLineDash([5, 4]); ctx.lineWidth = 1.2; ctx.strokeRect(sx - 4, sy - 4, ex - sx + 8, ey - sy + 8); ctx.setLineDash([]);
    if (B.sel.size === 1 && o.t !== 'path' && o.t !== 'arrow') { ctx.fillStyle = '#38E1F0'; ctx.fillRect(ex + 1, ey + 1, 8, 8); }
  }
  // точки привязки и «+» у фигуры под курсором
  const hv = B.hover && B.objs.get(B.hover);
  if (hv && canEdit() && (!B.drag || B.drag.kind === 'link' || B.drag.kind === 'lend')) {
    for (const pp of portsOf(hv)) { const [qx, qy] = W2S(pp.x, pp.y); ctx.beginPath(); ctx.arc(qx, qy, PORT_R - 1, 0, Math.PI * 2); ctx.fillStyle = '#38E1F0'; ctx.fill(); ctx.strokeStyle = bgc; ctx.lineWidth = 2.5; ctx.stroke(); }
    if (hv.t === 'shape' && !DG.isContainer(hv) && B.tool === 'select' && !B.drag) for (const q of plusOf(hv)) {
      ctx.beginPath(); ctx.arc(q.sx, q.sy, PLUS_R, 0, Math.PI * 2); ctx.fillStyle = 'rgba(20,23,48,.92)'; ctx.fill(); ctx.strokeStyle = 'rgba(56,225,240,.8)'; ctx.lineWidth = 1.5; ctx.stroke();
      ctx.beginPath(); ctx.moveTo(q.sx - 4.5, q.sy); ctx.lineTo(q.sx + 4.5, q.sy); ctx.moveTo(q.sx, q.sy - 4.5); ctx.lineTo(q.sx, q.sy + 4.5); ctx.strokeStyle = '#38E1F0'; ctx.lineWidth = 2; ctx.stroke();
    }
  }
  // направляющие и равные промежутки
  if (B.drag && B.drag.kind === 'move' && (B.guides.length || B.gaps.length)) {
    ctx.strokeStyle = '#FF5FA2'; ctx.fillStyle = '#FF5FA2'; ctx.lineWidth = 1; ctx.setLineDash([4, 3]);
    for (const g of B.guides) { const [ax, ay] = W2S(g[0], g[1]), [bx2, by2] = W2S(g[2], g[3]); ctx.beginPath(); ctx.moveTo(Math.round(ax) + 0.5, Math.round(ay) + 0.5); ctx.lineTo(Math.round(bx2) + 0.5, Math.round(by2) + 0.5); ctx.stroke(); }
    ctx.setLineDash([]); ctx.font = '700 11px Manrope, sans-serif';
    for (const g of B.gaps) {
      const [ax, ay] = W2S(g.x0, g.y0), [bx2, by2] = W2S(g.x1, g.y1); ctx.beginPath(); ctx.moveTo(ax, ay); ctx.lineTo(bx2, by2); ctx.stroke();
      const t = Math.round(g.len) + '', tw = ctx.measureText(t).width, mx = (ax + bx2) / 2, my = (ay + by2) / 2;
      ctx.fillRect(mx - tw / 2 - 5, my - 9, tw + 10, 18); ctx.fillStyle = '#fff'; ctx.textBaseline = 'middle'; ctx.fillText(t, mx - tw / 2, my + 0.5); ctx.fillStyle = '#FF5FA2';
    }
  }
  if (B.drag && B.drag.kind === 'marquee') { const m = B.drag; ctx.strokeStyle = 'rgba(56,225,240,.8)'; ctx.fillStyle = 'rgba(56,225,240,.08)'; const [sx, sy] = W2S(Math.min(m.x0, m.x1), Math.min(m.y0, m.y1)); const w = Math.abs(m.x1 - m.x0) * z, h = Math.abs(m.y1 - m.y0) * z; ctx.fillRect(sx, sy, w, h); ctx.strokeRect(sx, sy, w, h); }
  const ring = bgLight() ? 'rgba(20,22,40,.7)' : 'rgba(255,255,255,.7)';
  if (B.tool === 'brush' && B.mouse.in) { ctx.strokeStyle = ring; ctx.lineWidth = 1; ctx.beginPath(); ctx.arc(B.mouse.sx, B.mouse.sy, SET.bBrush || 12, 0, Math.PI * 2); ctx.stroke(); }
  if (B.tool === 'eraser' && B.mouse.in) { ctx.strokeStyle = ring; ctx.beginPath(); ctx.arc(B.mouse.sx, B.mouse.sy, 10, 0, Math.PI * 2); ctx.stroke(); }
  // курсоры других
  if (SET.bCursors !== false) for (const [n, c] of B.curs) {
    if (c.x === undefined) continue;
    const [sx, sy] = W2S(c.x, c.y); if (sx < -20 || sy < -20 || sx > B.W + 20 || sy > B.H + 20) continue;
    const col = curColor(n); ctx.fillStyle = col; ctx.beginPath(); ctx.moveTo(sx, sy); ctx.lineTo(sx + 13, sy + 7); ctx.lineTo(sx + 6, sy + 9); ctx.lineTo(sx + 3, sy + 16); ctx.closePath(); ctx.fill();
    const label = titleOf(n); ctx.font = '700 11px Manrope, sans-serif'; const tw = ctx.measureText(label).width; ctx.fillRect(sx + 10, sy + 14, tw + 12, 18); ctx.fillStyle = '#0B0C1A'; ctx.fillText(label, sx + 16, sy + 27);
  }
  el.z.textContent = Math.round(z * 100) + '%';
  stat();
}
function grid(ctx) {
  if (SET.bGrid === 'none') return;
  const d = B.dpr; let step = GRID * B.view.z; while (step < 12) step *= 4;
  const ox = (-B.view.x * B.view.z) % step, oy = (-B.view.y * B.view.z) % step;
  const light = bgLight();
  ctx.setTransform(d, 0, 0, d, 0, 0);
  if (SET.bGrid === 'grid') {
    ctx.strokeStyle = light ? 'rgba(30,34,70,.10)' : 'rgba(160,170,255,.07)'; ctx.lineWidth = 1; ctx.beginPath();
    for (let x = ox; x < B.W; x += step) { ctx.moveTo(Math.round(x) + 0.5, 0); ctx.lineTo(Math.round(x) + 0.5, B.H); }
    for (let y = oy; y < B.H; y += step) { ctx.moveTo(0, Math.round(y) + 0.5); ctx.lineTo(B.W, Math.round(y) + 0.5); }
    ctx.stroke(); return;
  }
  ctx.fillStyle = light ? 'rgba(30,34,70,.22)' : 'rgba(160,170,255,.18)';
  for (let x = ox; x < B.W; x += step) for (let y = oy; y < B.H; y += step) ctx.fillRect(x - 0.75, y - 0.75, 1.5, 1.5);
}
function drawObj(ctx, o, z, seen, exporting) {
  const b = box(o);
  if (!exporting && (b[2] - b[0]) * z < 1 && (b[3] - b[1]) * z < 1) return;
  ctx.globalAlpha = 1;
  switch (o.t) {
    case 'path': { const c = B.cache.get(o.id); if (!c.p) c.p = pathOf(o);
      ctx.strokeStyle = o.c || '#E8EAFB'; ctx.lineWidth = o.w || 3; ctx.globalAlpha = o.a || 1; ctx.lineCap = 'round'; ctx.lineJoin = 'round'; ctx.stroke(c.p); ctx.globalAlpha = 1; break; }
    case 'rect': ctx.strokeStyle = o.c; ctx.lineWidth = o.sw || 3; ctx.lineJoin = 'round'; if (o.f) { ctx.fillStyle = o.f; ctx.fillRect(o.x, o.y, o.w, o.h); } ctx.strokeRect(o.x, o.y, o.w, o.h); break;
    case 'arrow': { const w = o.sw || 3, a = Math.atan2(o.y2 - o.y, o.x2 - o.x), hl = Math.max(10, w * 4);
      ctx.strokeStyle = o.c; ctx.fillStyle = o.c; ctx.lineWidth = w; ctx.lineCap = 'round'; ctx.beginPath(); ctx.moveTo(o.x, o.y); ctx.lineTo(o.x2 - Math.cos(a) * hl * 0.6, o.y2 - Math.sin(a) * hl * 0.6); ctx.stroke();
      ctx.beginPath(); ctx.moveTo(o.x2, o.y2); ctx.lineTo(o.x2 - Math.cos(a - 0.45) * hl, o.y2 - Math.sin(a - 0.45) * hl); ctx.lineTo(o.x2 - Math.cos(a + 0.45) * hl, o.y2 - Math.sin(a + 0.45) * hl); ctx.closePath(); ctx.fill(); break; }
    case 'text': { if (B.edit_el === o.id) break; const c = B.cache.get(o.id), fs = o.fs || 20; ctx.fillStyle = o.c || '#E8EAFB'; ctx.font = fontOf(o, fs); ctx.textBaseline = 'top';
      textLines(ctx, o, c.lines || [], o.x, o.y, b[2] - b[0], fs, 1.3); break; }
    case 'note': { const c = B.cache.get(o.id), fs = o.fs || 16, w = o.w || 220, h = c.h;
      ctx.save(); ctx.shadowColor = 'rgba(0,0,0,.35)'; ctx.shadowBlur = 16 * (exporting ? 1 : Math.min(1, z)); ctx.shadowOffsetY = 6; ctx.fillStyle = o.bg || '#FFE58A'; ctx.fillRect(o.x, o.y, w, h); ctx.restore();
      if (B.edit_el === o.id) break;
      if (fs * z < 3 && !exporting) { ctx.fillStyle = 'rgba(58,46,0,.35)'; ctx.fillRect(o.x + 14, o.y + 14, w - 28, Math.min(h - 28, c.lines.length * fs * 1.35)); break; }
      ctx.fillStyle = o.c && o.c !== '#E8EAFB' ? o.c : '#2E2500'; ctx.font = fontOf(o, fs); ctx.textBaseline = 'top'; textLines(ctx, o, c.lines, o.x + 14, o.y + 14, w - 28, fs, 1.35); break; }
    case 'shape': drawShape(ctx, o, z, exporting); break;
    case 'link': drawLink(ctx, o, z, exporting); break;
    case 'img': { const sw = Math.abs(o.w) * z;
      seen && seen.add(o.src);
      if (!exporting && /\.gif$/.test(o.src || '') && sw >= 24) { B.gifVis.add(o.id); break; }
      const im = exporting || sw >= 24 ? image(o.src) : null;
      if (o.pdf) { ctx.fillStyle = '#FFFFFF'; ctx.fillRect(o.x, o.y, o.w, o.h); }
      if (im && im.ok) ctx.drawImage(im.img, o.x, o.y, o.w, o.h);
      else { ctx.fillStyle = 'rgba(160,170,255,.12)'; ctx.fillRect(o.x, o.y, o.w, o.h); ctx.strokeStyle = 'rgba(160,170,255,.3)'; ctx.lineWidth = 1 / z; ctx.strokeRect(o.x, o.y, o.w, o.h); }
      if (o.pdf && !exporting && sw >= 60) { // подпись страницы PDF
        ctx.fillStyle = 'rgba(11,12,26,.72)'; const fs = 12 / z; ctx.font = '600 ' + fs + 'px Manrope, sans-serif'; const label = o.pdf + ' · стр. ' + o.page;
        const tw = ctx.measureText(label).width; ctx.fillRect(o.x, o.y - fs * 1.7, tw + fs, fs * 1.5); ctx.fillStyle = '#E8EAFB'; ctx.textBaseline = 'top'; ctx.fillText(label, o.x + fs / 2, o.y - fs * 1.45);
      }
      break; }
  }
}
// ───────────── фигуры и связи ─────────────
/** Цвет текста фигуры: свой, иначе по заливке, иначе по фону доски. */
function shapeText(o) { if (o.tc) return o.tc; if (o.f) return isLightC(o.f) ? '#1B1D33' : '#E8EAFB'; return bgLight() ? '#1B1D33' : '#E8EAFB'; }
function drawShape(ctx, o, z, exporting) {
  const c = B.cache.get(o.id) || {}, p = DG.shapePath(o), sw = o.sw || 2, col = o.c || '#E8EAFB', cont = DG.isContainer(o);
  ctx.save(); ctx.globalAlpha = o.a || 1; ctx.lineJoin = 'round'; ctx.lineCap = 'round';
  if (cont) {
    ctx.fillStyle = o.f || (bgLight() ? 'rgba(30,34,70,.035)' : 'rgba(160,170,255,.045)'); ctx.fill(p);
    ctx.fillStyle = col; ctx.globalAlpha = (o.a || 1) * 0.12;
    if (o.k === 'frame') { ctx.beginPath(); ctx.roundRect(o.x, o.y, o.w, DG.HEAD, [10, 10, 0, 0]); ctx.fill(); } else ctx.fillRect(o.x, o.y, DG.LANE_HEAD, o.h);
    ctx.globalAlpha = o.a || 1;
  } else if (o.k === 'cloud') { ctx.strokeStyle = col; ctx.lineWidth = sw * 2; ctx.setLineDash(DG.dashOf(o.sd, sw)); ctx.stroke(p); ctx.fillStyle = o.f || bgColor(); ctx.fill(p); }
  else if (o.f) { ctx.fillStyle = o.f; ctx.fill(p); }
  if (o.k !== 'cloud') { ctx.strokeStyle = col; ctx.lineWidth = sw; ctx.setLineDash(DG.dashOf(o.sd, sw)); ctx.stroke(p); }
  ctx.setLineDash([]);
  if (o.text && B.edit_el !== o.id) {
    const fs = o.fs || 18, tb = DG.textBox(o), lines = c.lines || String(o.text).split('\n'), lh = 1.3;
    ctx.fillStyle = shapeText(o); ctx.font = fontOf(o, fs); ctx.textBaseline = 'top';
    if (fs * z < 3 && !exporting) { ctx.globalAlpha = 0.3; ctx.fillRect(tb[0] + tb[2] * 0.2, tb[1] + tb[3] / 2 - fs / 2, tb[2] * 0.6, fs); }
    else if (o.k === 'lane') {
      ctx.save(); ctx.translate(tb[0] + tb[2] / 2, tb[1] + tb[3] / 2); ctx.rotate(-Math.PI / 2); ctx.textAlign = 'center';
      lines.forEach((l, i) => ctx.fillText(l, 0, (i - lines.length / 2) * fs * lh)); ctx.restore();
    } else {
      const th = lines.length * fs * lh, va = o.k === 'frame' ? 'middle' : o.va || 'middle';
      const y = va === 'top' ? tb[1] + 4 : va === 'bottom' ? tb[1] + tb[3] - th : tb[1] + (tb[3] - th) / 2;
      textLines(ctx, o.fa || o.k === 'frame' ? o : { ...o, fa: 'center' }, lines, tb[0], y + fs * 0.08, tb[2], fs, lh);
    }
  }
  ctx.restore();
}
function drawLink(ctx, o, z, exporting) {
  const c = B.cache.get(o.id); if (!c || !c.g) return;
  const g = c.g, sw = o.sw || 2, col = o.c || '#E8EAFB', ha = o.ha || 'none', hb = o.hb === undefined ? 'arrow' : o.hb;
  const ia = DG.headInset(ha, sw), ib = DG.headInset(hb, sw);
  ctx.save(); ctx.strokeStyle = col; ctx.lineWidth = sw; ctx.lineCap = 'round'; ctx.lineJoin = 'round'; ctx.setLineDash(DG.dashOf(o.sd, sw)); ctx.globalAlpha = o.a || 1;
  const pa = [g.a[0] - Math.cos(g.a[2]) * ia, g.a[1] - Math.sin(g.a[2]) * ia], pb = [g.b[0] - Math.cos(g.b[2]) * ib, g.b[1] - Math.sin(g.b[2]) * ib];
  ctx.beginPath();
  if (g.path.k === 'curve') { const [, c1, c2] = g.path.bez; ctx.moveTo(pa[0], pa[1]); ctx.bezierCurveTo(c1[0], c1[1], c2[0], c2[1], pb[0], pb[1]); }
  else { const pts = g.poly; ctx.moveTo(pa[0], pa[1]); for (let i = 1; i < pts.length - 1; i++) ctx.lineTo(pts[i][0], pts[i][1]); ctx.lineTo(pb[0], pb[1]); }
  ctx.stroke(); ctx.setLineDash([]);
  const bg = bgColor();
  DG.drawHead(ctx, ha, g.a[0], g.a[1], g.a[2], sw, col, bg); DG.drawHead(ctx, hb, g.b[0], g.b[1], g.b[2], sw, col, bg);
  if (o.text && c.lb && B.edit_el !== o.id) {
    const fs = o.fs || 15, lb = c.lb;
    ctx.fillStyle = bg; ctx.beginPath(); ctx.roundRect(lb[0], lb[1], lb[2] - lb[0], lb[3] - lb[1], Math.min(fs, (lb[3] - lb[1]) / 2)); ctx.fill(); ctx.strokeStyle = col; ctx.lineWidth = Math.max(1, sw * 0.6); ctx.stroke();
    if (fs * z >= 3 || exporting) { ctx.fillStyle = o.tc || col; ctx.font = fontOf(o, fs); ctx.textBaseline = 'top'; textLines(ctx, o.fa ? o : { ...o, fa: 'center' }, c.lines, lb[0] + fs / 2, lb[1] + fs * 0.3, lb[2] - lb[0] - fs, fs, 1.3); }
  }
  ctx.restore();
}
/** Значок фигуры для кнопок (картинка с холста). */
const ICONS = new Map();
function shapeIcon(k, px = 22) {
  const key = k + px; if (ICONS.has(key)) return ICONS.get(key);
  const cv = document.createElement('canvas'), d = 2; cv.width = cv.height = px * d; const ctx = cv.getContext('2d');
  const [w0, h0] = DG.shapeSize(k), sc = (px - 4) / Math.max(w0, h0), w = w0 * sc, h = h0 * sc;
  const o = { t: 'shape', k, x: (px - w) / 2, y: (px - h) / 2, w, h };
  ctx.scale(d, d); ctx.strokeStyle = '#E8EAFB'; ctx.lineWidth = 1.5; ctx.lineJoin = 'round'; const p = DG.shapePath(o);
  if (k === 'cloud') { ctx.lineWidth = 3; ctx.stroke(p); ctx.fillStyle = '#141730'; ctx.fill(p); } else ctx.stroke(p);
  const html = '<img class="shic" alt="" src="' + cv.toDataURL() + '" width="' + px + '" height="' + px + '">'; ICONS.set(key, html); return html;
}
// строки текста с выравниванием и подчёркиванием
function textLines(ctx, o, lines, x, y, W, fs, lh) {
  lines.forEach((l, i) => {
    const lw = o.fa || o.fu ? ctx.measureText(l).width : 0, dx = o.fa === 'center' ? (W - lw) / 2 : o.fa === 'right' ? W - lw : 0, ly = y + i * fs * lh;
    ctx.fillText(l, x + dx, ly);
    if (o.fu && l) ctx.fillRect(x + dx, ly + fs * 1.08, lw, Math.max(1, fs / 14));
  });
}
// шрифт догрузился — перемерить тексты (ширина строк другая)
function remeasure() { for (const o of B.objs.values()) if (hasText(o) && o.text) setObj(o, true); dirty(); }
if (typeof document !== 'undefined' && document.fonts) {
  document.fonts.addEventListener('loadingdone', () => remeasure());
  for (const f of FONTS.slice(1)) for (const w of ['500', '700']) document.fonts.load(w + " 20px '" + f[2] + "'", 'AaЖж').catch(() => {});
}
// гифки — живые <img> поверх холста (холст рисует только первый кадр): только видимые, остальные убираются
function gifs() {
  const keep = new Set(), top = B.full ? 52 : 0;
  for (const id of B.gifVis) {
    const o = B.objs.get(id); if (!o) continue; keep.add(id);
    let im = el.gifs.querySelector('[data-g="' + CSS.escape(id) + '"]');
    if (!im) { im = document.createElement('img'); im.dataset.g = id; im.alt = ''; im.draggable = false; el.gifs.appendChild(im); }
    if (im.dataset.src !== o.src) { im.src = imgUrl(o.src); im.dataset.src = o.src; }
    const [sx, sy] = W2S(Math.min(o.x, o.x + o.w), Math.min(o.y, o.y + o.h));
    im.style.cssText = 'left:' + sx + 'px;top:' + (sy + top) + 'px;width:' + Math.abs(o.w) * B.view.z + 'px;height:' + Math.abs(o.h) * B.view.z + 'px;z-index:' + (1 + layerIdx(layerOf(o)));
  }
  el.gifs.querySelectorAll('img').forEach(im => { if (!keep.has(im.dataset.g)) im.remove(); });
}
function pathOf(o) {
  const p = new Path2D(), pts = o.pts || [], x = o.x, y = o.y;
  if (pts.length < 4) { p.moveTo(x + (pts[0] || 0), y + (pts[1] || 0)); p.lineTo(x + (pts[0] || 0) + 0.01, y + (pts[1] || 0)); return p; }
  p.moveTo(x + pts[0], y + pts[1]);
  for (let i = 2; i < pts.length - 2; i += 2) { const mx = (pts[i] + pts[i + 2]) / 2, my = (pts[i + 1] + pts[i + 3]) / 2; p.quadraticCurveTo(x + pts[i], y + pts[i + 1], x + mx, y + my); }
  p.lineTo(x + pts[pts.length - 2], y + pts[pts.length - 1]);
  return p;
}
// пул картинок: грузятся, когда видны; лишние (давно невидимые) выгружаются
function image(id) {
  let im = B.imgs.get(id); if (im) return im;
  if (B.imgs.size >= IMG_POOL) {
    let old = null; for (const [k, v] of B.imgs) if (!old || v.last < old[1].last) old = [k, v];
    if (old && Date.now() - old[1].last > 1000) { old[1].img.src = ''; B.imgs.delete(old[0]); } else return null;
  }
  const img = new Image(); im = { img, ok: false, last: Date.now() }; B.imgs.set(id, im);
  img.onload = () => { im.ok = true; dirty(); }; img.onerror = () => { setTimeout(() => B.imgs.delete(id), 5000); };
  img.src = imgUrl(id);
  return im;
}
function stat() {
  el.xy.textContent = 'x ' + Math.round(B.mouse.wx || 0) + ' · y ' + Math.round(B.mouse.wy || 0) + ' · на экране ' + B.drawn + ' из ' + B.objs.size;
  if (!el.cfg.classList.contains('hide')) {
    let bytes = 0; if (B.objs.size < 20000) for (const o of B.objs.values()) bytes += JSON.stringify(o).length;
    el.stats.innerHTML = '<div class="row"><span class="muted grow">Объектов</span><span>' + B.objs.size + (bytes ? ' · ' + size(bytes) : '') + '</span></div><div class="row"><span class="muted grow">Нарисовано на экране</span><span>' + B.drawn + '</span></div><div class="row"><span class="muted grow">Картинок в памяти</span><span>' + [...B.imgs.values()].filter(x => x.ok).length + ' из ' + new Set([...B.objs.values()].filter(o => o.t === 'img').map(o => o.src)).size + '</span></div><div class="row"><span class="muted grow">Слоёв</span><span>' + B.layers.length + '</span></div>';
  }
}
function mini() {
  const c = el.mini, ctx = c.getContext('2d'), W = c.width, H = c.height; ctx.clearRect(0, 0, W, H);
  const [vx0, vy0] = S2W(0, 0), [vx1, vy1] = S2W(B.W, B.H); let b = worldBounds();
  b = b ? [Math.min(b[0], vx0), Math.min(b[1], vy0), Math.max(b[2], vx1), Math.max(b[3], vy1)] : [vx0, vy0, vx1, vy1];
  const pad = 40, s = Math.min((W - 16) / (b[2] - b[0] + pad * 2), (H - 16) / (b[3] - b[1] + pad * 2)); B.mm = { b, s, pad };
  const X = x => 8 + (x - b[0] + pad) * s, Y = y => 8 + (y - b[1] + pad) * s;
  let i = 0; const step = Math.max(1, Math.floor(B.objs.size / 4000));
  for (const o of B.objs.values()) {
    if (i++ % step || !layerShown(layerOf(o))) continue;
    const x = box(o); ctx.fillStyle = o.t === 'img' ? 'rgba(160,170,255,.45)' : o.t === 'note' ? 'rgba(255,229,138,.7)' : (o.c || '#E8EAFB') + 'aa';
    ctx.fillRect(X(x[0]), Y(x[1]), Math.max(1.5, (x[2] - x[0]) * s), Math.max(1.5, (x[3] - x[1]) * s));
  }
  ctx.strokeStyle = 'rgba(232,234,251,.8)'; ctx.lineWidth = 1.5; ctx.strokeRect(X(vx0), Y(vy0), (vx1 - vx0) * s, (vy1 - vy0) * s);
}
function miniNav(e) {
  if (!B.mm) return; const r = el.mini.getBoundingClientRect(), { b, s, pad } = B.mm;
  const wx = (e.clientX - r.left - 8) / s + b[0] - pad, wy = (e.clientY - r.top - 8) / s + b[1] - pad;
  B.view.x = wx - B.W / 2 / B.view.z; B.view.y = wy - B.H / 2 / B.view.z; B.follow = ''; dirty(); mini();
}

// ───────────── ввод ─────────────
function pt(e) { const r = el.cv.getBoundingClientRect(); const sx = e.clientX - r.left, sy = e.clientY - r.top; const [wx, wy] = S2W(sx, sy); return { sx, sy, wx, wy }; }
/** Объект под точкой — только в активном слое (стирать и выделять вне слоя нельзя). */
function hit(wx, wy, rad) {
  const r = rad / B.view.z, list = query(wx - r, wy - r, wx + r, wy + r).filter(inActive);
  for (let i = list.length - 1; i >= 0; i--) {
    const o = list[i];
    if (o.t === 'path') { const p = o.pts || [], lim = r + (o.w || 3) / 2; for (let k = 0; k < p.length - 2; k += 2) if (segDist(wx, wy, o.x + p[k], o.y + p[k + 1], o.x + p[k + 2], o.y + p[k + 3]) <= lim) return o; if (p.length === 2 && Math.hypot(wx - o.x - p[0], wy - o.y - p[1]) <= lim) return o; continue; }
    if (o.t === 'arrow') { if (segDist(wx, wy, o.x, o.y, o.x2, o.y2) <= r + (o.sw || 3)) return o; continue; }
    if (o.t === 'link') { if (hitLink(o, wx, wy, r)) return o; continue; }
    if (DG.isContainer(o)) { if (hitCont(o, wx, wy, r)) return o; continue; }
    const b = box(o); if (wx >= b[0] - r && wx <= b[2] + r && wy >= b[1] - r && wy <= b[3] + r) return o;
  }
  return null;
}
function hitLink(o, wx, wy, r) {
  const c = B.cache.get(o.id) || (box(o), B.cache.get(o.id)); if (!c || !c.g) return false;
  if (c.lb && wx >= c.lb[0] && wx <= c.lb[2] && wy >= c.lb[1] && wy <= c.lb[3]) return true;
  const p = c.g.poly, lim = r + (o.sw || 2) + 3 / B.view.z;
  for (let i = 1; i < p.length; i++) if (segDist(wx, wy, p[i - 1][0], p[i - 1][1], p[i][0], p[i][1]) <= lim) return true;
  return false;
}
/** Рамку и дорожку берут за заголовок или край — внутри них выделяют то, что лежит на них. */
function hitCont(o, wx, wy, r) {
  const m = r + 5 / B.view.z, x0 = o.x, y0 = o.y, x1 = o.x + o.w, y1 = o.y + o.h;
  if (wx < x0 - m || wx > x1 + m || wy < y0 - m || wy > y1 + m) return false;
  if (o.k === 'frame' && wy <= y0 + DG.HEAD) return true;
  if (o.k === 'lane' && wx <= x0 + DG.LANE_HEAD) return true;
  return wx < x0 + m || wx > x1 - m || wy < y0 + m || wy > y1 - m;
}
function segDist(px, py, ax, ay, bx, by) { const dx = bx - ax, dy = by - ay, l = dx * dx + dy * dy; let t = l ? ((px - ax) * dx + (py - ay) * dy) / l : 0; t = Math.max(0, Math.min(1, t)); return Math.hypot(px - ax - t * dx, py - ay - t * dy); }
function layerHiddenWarn() { if (B.hidden.has(B.layer)) { toast('Активный слой скрыт — покажите его, чтобы рисовать', 'bad'); return true; } return false; }
function down(e) {
  if (B.edit_el) el.text.blur();
  el.cv.setPointerCapture(e.pointerId); el.bd.focus({ preventScroll: true });
  const p = pt(e); closePop();
  if (e.button === 1 || e.button === 2 || B.space || B.tool === 'hand' || (!canEdit() && B.tool !== 'select')) { B.drag = { kind: 'pan', sx: p.sx, sy: p.sy, vx: B.view.x, vy: B.view.y }; B.follow = ''; el.cv.style.cursor = 'grabbing'; return; }
  const t = B.tool, z = B.view.z;
  if (t === 'select') {
    if (canEdit()) {
      const h = B.hover && B.objs.get(B.hover);
      if (h) {
        const ps = h.t === 'shape' && plusAt(h, p.sx, p.sy); if (ps) { quickAdd(h, ps); return; }
        const pp = portAt(h, p.sx, p.sy); if (pp) { startLink(p, { id: h.id, s: pp, x: Math.round(p.wx), y: Math.round(p.wy) }, true); return; }
      }
      if (B.sel.size === 1) {
        const o = B.objs.get([...B.sel][0]), c = o && o.t === 'link' && B.cache.get(o.id);
        if (c && c.g) {
          for (const which of ['a', 'b']) { const [qx, qy] = W2S(c.g[which][0], c.g[which][1]); if (Math.hypot(qx - p.sx, qy - p.sy) <= 10) { B.drag = { kind: 'lend', id: o.id, which, before: copy(o) }; return; } }
          if (c.lb && p.wx >= c.lb[0] && p.wx <= c.lb[2] && p.wy >= c.lb[1] && p.wy <= c.lb[3]) { B.drag = { kind: 'llabel', id: o.id, before: copy(o) }; return; }
        }
      }
    }
    if (B.sel.size === 1) {
      const o = B.objs.get([...B.sel][0]);
      if (o && o.t !== 'path' && o.t !== 'arrow' && o.t !== 'link' && canEdit()) { const b = box(o), [ex, ey] = W2S(b[2], b[3]); if (Math.abs(p.sx - ex - 5) < 9 && Math.abs(p.sy - ey - 5) < 9) { B.drag = { kind: 'resize', id: o.id, before: copy(o), x0: p.wx, y0: p.wy }; return; } }
    }
    const o = hit(p.wx, p.wy, 6);
    if (o) {
      const grp = o.g ? [...B.objs.values()].filter(x => x.g === o.g && inActive(x)).map(x => x.id) : [o.id];
      if (e.shiftKey) { const on = B.sel.has(o.id); for (const id of grp) on ? B.sel.delete(id) : B.sel.add(id); } else if (!B.sel.has(o.id)) { B.sel.clear(); for (const id of grp) B.sel.add(id); }
      if (canEdit()) B.drag = moveDrag(p);
      dirty(); return;
    }
    if (!e.shiftKey) B.sel.clear();
    B.drag = { kind: 'marquee', x0: p.wx, y0: p.wy, x1: p.wx, y1: p.wy }; dirty(); return;
  }
  if (layerHiddenWarn()) return;
  if (t === 'pen' || t === 'marker' || t === 'brush') {
    const w = t === 'brush' ? (2 * (SET.bBrush || 12)) / z : ((SIZES[B.size] || 4) * (t === 'marker' ? 4 : 1)) / Math.max(0.25, Math.min(4, z));
    const o = { id: newId(), t: 'path', L: B.layer, x: Math.round(p.wx * 10) / 10, y: Math.round(p.wy * 10) / 10, pts: [0, 0], c: B.color, w: Math.round(w * 10) / 10, z: Date.now() + Math.random() };
    if (t === 'marker') o.a = 0.35;
    setObj(o); send({ t: 'put', o }); B.drag = { kind: 'draw', id: o.id, lx: p.wx, ly: p.wy }; return;
  }
  if (t === 'eraser') { B.drag = { kind: 'erase', before: new Map() }; erase(p); return; }
  if (t === 'link') {
    const h = attTarget(p.wx, p.wy); let a;
    if (h) { const pp = portAt(h, p.sx, p.sy); a = { id: h.id, x: Math.round(p.wx), y: Math.round(p.wy) }; if (pp) a.s = pp; } else a = { x: snap(p.wx), y: snap(p.wy) };
    startLink(p, a, false); return;
  }
  if (t === 'shape') {
    const x = snap(p.wx), y = snap(p.wy), o = { id: newId(), t: 'shape', ...shapeDef(), L: B.layer, x, y, w: 0, h: 0, z: Date.now() };
    setObj(o); B.drag = { kind: 'shape', id: o.id, x0: x, y0: y }; return;
  }
  if (t === 'rect' || t === 'arrow') {
    const x = snap(p.wx), y = snap(p.wy), sw = (SIZES[B.size] || 4) / Math.max(0.25, Math.min(4, z));
    const o = t === 'rect' ? { id: newId(), t: 'rect', L: B.layer, x, y, w: 0, h: 0, c: B.color, sw, z: Date.now() } : { id: newId(), t: 'arrow', L: B.layer, x, y, x2: x, y2: y, c: B.color, sw, z: Date.now() };
    setObj(o); B.drag = { kind: 'shape', id: o.id, x0: x, y0: y }; return;
  }
  if (t === 'text' || t === 'note') {
    const d = txtDef(), fs = Math.round((t === 'text' ? d.fs : Math.round(d.fs * 0.75)) / Math.max(0.2, Math.min(4, z)));
    const st = styleOf(d);
    const o = t === 'text' ? { id: newId(), t: 'text', L: B.layer, x: snap(p.wx), y: snap(p.wy), text: '', c: B.color, fs, ...st, z: Date.now() }
      : { id: newId(), t: 'note', L: B.layer, x: snap(p.wx), y: snap(p.wy), w: Math.round(220 / Math.max(0.2, Math.min(4, z))), text: '', bg: NOTE_BG[B.color] || '#FFE58A', fs, ...st, z: Date.now() };
    setObj(o); editText(o, true); setTool('select'); return;
  }
  if (t === 'image') { pickFiles(p.wx, p.wy); setTool('select'); }
}
function move(e) {
  const p = pt(e); B.mouse = { ...p, in: true }; if (B.tool === 'eraser' || B.tool === 'brush') dirty();
  if (B.vis && B.room && Date.now() - B.lastCur > Math.max(80, +SET.bSync || 50)) {
    B.lastCur = Date.now();
    send({ t: 'cur', x: Math.round(p.wx), y: Math.round(p.wy), vx: Math.round(B.view.x + B.W / 2 / B.view.z), vy: Math.round(B.view.y + B.H / 2 / B.view.z), vz: +B.view.z.toFixed(3) });
  }
  const d = B.drag; if (!d) { hoverAt(p); stat(); return; }
  if (d.kind === 'pan') { B.view.x = d.vx - (p.sx - d.sx) / B.view.z; B.view.y = d.vy - (p.sy - d.sy) / B.view.z; dirty(); return; }
  if (d.kind === 'draw') {
    const o = B.objs.get(d.id); if (!o) return; if (Math.hypot(p.wx - d.lx, p.wy - d.ly) * B.view.z < 1.5) return; d.lx = p.wx; d.ly = p.wy;
    o.pts.push(Math.round((p.wx - o.x) * 10) / 10, Math.round((p.wy - o.y) * 10) / 10); setObj(o); send({ t: 'put', o });
    if (o.pts.length > 16000) up(e);
    return;
  }
  if (d.kind === 'erase') { erase(p); return; }
  if (d.kind === 'shape') {
    const o = B.objs.get(d.id); if (!o) return;
 let x = snap(p.wx), y = snap(p.wy);
    if (o.t === 'rect' || o.t === 'shape') { if (e.shiftKey) { const s = Math.max(Math.abs(x - d.x0), Math.abs(y - d.y0)); x = d.x0 + Math.sign(x - d.x0 || 1) * s; y = d.y0 + Math.sign(y - d.y0 || 1) * s; } o.x = Math.min(d.x0, x); o.y = Math.min(d.y0, y); o.w = Math.abs(x - d.x0); o.h = Math.abs(y - d.y0); }
    else { o.x2 = x; o.y2 = y; }
    setObj(o); send({ t: 'put', o }); return;
  }
  if (d.kind === 'move') {
    let dx = snap(p.wx - d.x0), dy = snap(p.wy - d.y0);
    B.guides = []; B.gaps = [];
    if (!e.altKey && d.mb && d.cands.length) [dx, dy] = guide(d, dx, dy);
    for (const [id, b0] of d.before) { const o = B.objs.get(id); if (!o || !b0) continue; shiftTo(o, b0, dx, dy); setObj(o); send({ t: 'put', o }); }
    return;
  }
  if (d.kind === 'link' || d.kind === 'lend') {
    const o = B.objs.get(d.id); if (!o) return; const which = d.kind === 'link' ? 'b' : d.which;
    const tg = attTarget(p.wx, p.wy, new Set([o.id]));
    if (tg) { const s0 = portAt(tg, p.sx, p.sy, 16); o[which] = { id: tg.id, x: Math.round(p.wx), y: Math.round(p.wy) }; if (s0) o[which].s = s0; B.hover = tg.id; }
    else { o[which] = { x: Math.round(p.wx), y: Math.round(p.wy) }; B.hover = null; }
    setObj(o); if (d.kind === 'lend') send({ t: 'put', o }); return;
  }
  if (d.kind === 'llabel') { const o = B.objs.get(d.id), c = o && B.cache.get(o.id); if (!c || !c.g) return; o.lt = Math.round(DG.nearT(c.g.poly, p.wx, p.wy) * 1000) / 1000; setObj(o); send({ t: 'put', o }); return; }
  if (d.kind === 'resize') {
    const o = B.objs.get(d.id), b0 = d.before; if (!o) return; const dx = p.wx - d.x0, dy = p.wy - d.y0;
    if (o.t === 'text') { const k = Math.max(0.2, 1 + dx / Math.max(60, (b0.fs || 20) * Math.max(...String(b0.text || ' ').split('\n').map(l => l.length)) * 0.55)); o.fs = Math.max(6, Math.round((b0.fs || 20) * k)); }
    else if (o.t === 'note') { o.w = Math.max(80, snap(b0.w + dx)); o.h = Math.max(0, (b0.h || 0) + dy); }
    else { const w = Math.max(8, snap(b0.w + dx)); let h = Math.max(8, snap(b0.h + dy)); if ((o.t === 'img' && !e.shiftKey) || (o.t === 'shape' && e.shiftKey)) h = (w * b0.h) / b0.w; o.w = w; o.h = h; }
    setObj(o); send({ t: 'put', o }); return;
  }
  if (d.kind === 'marquee') { d.x1 = p.wx; d.y1 = p.wy; dirty(); }
}
function up(e) {
  const d = B.drag; B.drag = null; el.cv.style.cursor = ''; if (!d) return;
  if (d.kind === 'move') { B.guides = []; B.gaps = []; dirty(); }
  if (d.kind === 'link') { linkDone(d, pt(e)); return; }
  if (d.kind === 'lend' || d.kind === 'llabel') { const o = B.objs.get(d.id); B.hover = null; if (o) { send({ t: 'put', o }); hist(new Map([[d.id, d.before]]), new Map([[d.id, copy(o)]])); } return; }
  if (d.kind === 'draw') {
    const o = B.objs.get(d.id); if (!o) return;
    if (SET.bSmooth !== false && o.pts.length > 6) o.pts = simplify(o.pts, 0.6 / B.view.z);
    setObj(o); send({ t: 'put', o }); hist(new Map([[o.id, null]]), new Map([[o.id, copy(o)]])); return;
  }
  if (d.kind === 'erase') { if (d.before.size) hist(d.before, new Map([...d.before.keys()].map(id => [id, null]))); return; }
  if (d.kind === 'shape') {
    const o = B.objs.get(d.id); if (!o) return;
    if (o.t === 'shape') {
      if (o.w * B.view.z < 8 && o.h * B.view.z < 8) { const kz = Math.max(0.25, Math.min(4, B.view.z)), [w, h] = DG.shapeSize(o.k); o.w = Math.round(w / kz); o.h = Math.round(h / kz); o.x = Math.round(d.x0 - o.w / 2); o.y = Math.round(d.y0 - o.h / 2); }
      setObj(o); send({ t: 'put', o }); hist(new Map([[o.id, null]]), new Map([[o.id, copy(o)]])); setTool('select'); B.sel = new Set([o.id]); dirty(); return;
    }
    const tiny = o.t === 'rect' ? o.w * B.view.z < 4 && o.h * B.view.z < 4 : Math.hypot(o.x2 - o.x, o.y2 - o.y) * B.view.z < 6;
    if (tiny) { delObj(o.id); send({ t: 'del', ids: [o.id] }); return; }
    send({ t: 'put', o }); hist(new Map([[o.id, null]]), new Map([[o.id, copy(o)]])); return;
  }
  if (d.kind === 'move') { const after = new Map([...d.before.keys()].map(id => [id, copy(B.objs.get(id))])); if ([...after].some(([id, o]) => o && (o.x !== d.before.get(id).x || o.y !== d.before.get(id).y))) hist(d.before, after); return; }
  if (d.kind === 'resize') { hist(new Map([[d.id, d.before]]), new Map([[d.id, copy(B.objs.get(d.id))]])); return; }
  if (d.kind === 'marquee') {
    const x0 = Math.min(d.x0, d.x1), x1 = Math.max(d.x0, d.x1), y0 = Math.min(d.y0, d.y1), y1 = Math.max(d.y0, d.y1);
    if ((x1 - x0) * B.view.z > 3) for (const o of query(x0, y0, x1, y1)) { if (!inActive(o)) continue; const b = box(o); if (b[0] >= x0 && b[2] <= x1 && b[1] >= y0 && b[3] <= y1) B.sel.add(o.id); }
    dirty();
  }
}
// ───────────── диаграммы: привязка, «+», направляющие ─────────────
const PORT_R = 7, PLUS_OFF = 30, PLUS_R = 10;
const kzoom = () => Math.max(0.25, Math.min(4, B.view.z));
function portsOf(o) { const b = box(o); return ['t', 'r', 'b', 'l'].map(sd => { const [x, y] = DG.portPt(o, b, sd); return { s: sd, x, y }; }); }
function plusOf(o) { return portsOf(o).map(pp => { const [dx, dy] = DG.SIDES[pp.s], [sx, sy] = W2S(pp.x, pp.y); return { s: pp.s, sx: sx + dx * PLUS_OFF, sy: sy + dy * PLUS_OFF }; }); }
function portAt(o, sx, sy, rad = PORT_R + 4) { for (const pp of portsOf(o)) { const [qx, qy] = W2S(pp.x, pp.y); if (Math.hypot(qx - sx, qy - sy) <= rad) return pp.s; } return null; }
function plusAt(o, sx, sy) { if (DG.isContainer(o)) return null; for (const q of plusOf(o)) if (Math.hypot(q.sx - sx, q.sy - sy) <= PLUS_R + 3) return q.s; return null; }
/** К чему привязать связь в этой точке (активный слой; рамку — за заголовок или край). */
function attTarget(wx, wy, skip) {
  const r = 4 / B.view.z, list = query(wx - r, wy - r, wx + r, wy + r).filter(o => inActive(o) && DG.attachable(o) && !(skip && skip.has(o.id)));
  for (let i = list.length - 1; i >= 0; i--) { const o = list[i], b = box(o); if (wx < b[0] - r || wx > b[2] + r || wy < b[1] - r || wy > b[3] + r) continue; if (DG.isContainer(o) && !hitCont(o, wx, wy, r)) continue; return o; }
  return null;
}
/** Наведение: у фигуры показываются точки привязки и «+» (только пока курсор рядом). */
function hoverAt(p) {
  if (!canEdit() || !B.vis || !(B.tool === 'select' || B.tool === 'link') || B.space) { if (B.hover) { B.hover = null; dirty(); } return; }
  let h = B.hover ? B.objs.get(B.hover) : null;
  if (h) { const b = box(h), m = (PLUS_OFF + PLUS_R + 4) / B.view.z; if (!inActive(h) || p.wx < b[0] - m || p.wx > b[2] + m || p.wy < b[1] - m || p.wy > b[3] + m) h = null; }
  const t = attTarget(p.wx, p.wy); if (t) h = t;
  const id = h ? h.id : null; if (id !== B.hover) { B.hover = id; dirty(); }
  el.cv.style.cursor = h && B.tool === 'select' && h.t === 'shape' && plusAt(h, p.sx, p.sy) ? 'pointer' : h && portAt(h, p.sx, p.sy) ? 'crosshair' : '';
}
/** Стиль новой фигуры и связи: последний выбранный (полоса свойств без выделения). */
function shapeDef() {
  const d = SET.bShapeSt || {}, kz = kzoom(), cont = B.shapeK === 'frame' || B.shapeK === 'lane';
  const o: any = { k: B.shapeK, c: B.color, f: cont ? '' : d.f !== undefined ? d.f : bgLight() ? '#FFFFFF' : '#1C2140', sw: Math.round(((d.sw || 2) / kz) * 10) / 10, fs: Math.round(18 / kz) };
  if (d.sd) o.sd = d.sd; if (cont) o.fb = true;
  return o;
}
function linkDef() {
  const d = SET.bLinkSt || {}, kz = kzoom();
  const o: any = { k: d.k || 'straight', ha: d.ha || 'none', hb: d.hb || 'arrow', c: B.color, sw: Math.round(((d.sw || 2) / kz) * 10) / 10, fs: Math.round(15 / kz) };
  if (d.sd) o.sd = d.sd;
  return o;
}
function startLink(p, a, fromPort) {
  const o = { id: newId(), t: 'link', L: B.layer, a, b: { x: Math.round(p.wx), y: Math.round(p.wy) }, ...linkDef(), z: Date.now() };
  setObj(o); B.drag = { kind: 'link', id: o.id, from: a.id || null, fromPort, sx: p.sx, sy: p.sy };
}
function linkDone(d, p) {
  const o = B.objs.get(d.id); B.hover = null; if (!o) return;
  if (Math.hypot(p.sx - d.sx, p.sy - d.sy) < 8 || (o.a.id && o.a.id === o.b.id && (o.a.s || '') === (o.b.s || ''))) { delObj(o.id); return; }
  if (!o.b.id && d.fromPort) { // из точки в пустое место — новая такая же фигура
    const src = B.objs.get(d.from);
    if (src && src.t === 'shape' && !DG.isContainer(src)) {
      const n = cloneShape(src); n.x = Math.round(o.b.x - n.w / 2); n.y = Math.round(o.b.y - n.h / 2);
      o.b = { id: n.id, x: o.b.x, y: o.b.y }; delObj(o.id);
      change(new Map([[n.id, null], [o.id, null]]), new Map([[n.id, n], [o.id, o]])); setTool('select'); B.sel = new Set([n.id]); editText(n, false); return;
    }
  }
  delObj(o.id); change(new Map([[o.id, null]]), new Map([[o.id, o]]));
}
function cloneShape(src) { const n = copy(src); n.id = newId(); delete n.text; delete n.g; delete n.by; n.z = Date.now(); return n; }
/** «+» у стороны (и Tab): такая же фигура рядом, сразу со связью; если место занято — сдвинуть. */
function quickAdd(src, side) {
  if (!src || src.t !== 'shape' || DG.isContainer(src) || !canEdit()) return;
  const n = cloneShape(src), gap = Math.max(50, 70 / kzoom()), [dx] = DG.SIDES[side];
  n.x = side === 'r' ? src.x + src.w + gap : side === 'l' ? src.x - gap - n.w : src.x;
  n.y = side === 'b' ? src.y + src.h + gap : side === 't' ? src.y - gap - n.h : src.y;
  for (let i = 0; i < 12; i++) { if (!query(n.x + 1, n.y + 1, n.x + n.w - 1, n.y + n.h - 1).some(o => o.id !== src.id && inActive(o) && DG.attachable(o) && !DG.isContainer(o))) break; if (dx) n.y += n.h + 30; else n.x += n.w + 30; }
  const opp = { t: 'b', b: 't', l: 'r', r: 'l' }[side];
  const l = { id: newId(), t: 'link', L: src.L || B.layer, a: { id: src.id, s: side }, b: { id: n.id, s: opp }, ...linkDef(), z: Date.now() + 1 };
  change(new Map([[n.id, null], [l.id, null]]), new Map([[n.id, n], [l.id, l]]));
  B.lastSide = side; B.hover = null; setTool('select'); B.sel = new Set([n.id]); editText(n, false);
}
/** Перетаскивание выделенного: рамки везут то, что на них лежит; рядом — кандидаты для направляющих. */
function moveDrag(p) {
  const before = new Map([...B.sel].map(id => [id, copy(B.objs.get(id))]));
  for (const id of [...before.keys()]) {
    const c = B.objs.get(id); if (!DG.isContainer(c)) continue;
    for (const o of query(c.x, c.y, c.x + c.w, c.y + c.h, true)) { if (before.has(o.id) || layerOf(o) !== layerOf(c) || o.t === 'link') continue; const b = box(o); if (b[0] >= c.x - 1 && b[1] >= c.y - 1 && b[2] <= c.x + c.w + 1 && b[3] <= c.y + c.h + 1) before.set(o.id, copy(o)); }
  }
  let mb = null; for (const id of before.keys()) { const o = B.objs.get(id); if (!o || o.t === 'link') continue; const b = box(o); mb = mb ? [Math.min(mb[0], b[0]), Math.min(mb[1], b[1]), Math.max(mb[2], b[2]), Math.max(mb[3], b[3])] : b.slice(); }
  const [x0, y0] = S2W(-300, -300), [x1, y1] = S2W(B.W + 300, B.H + 300);
  const cands = mb ? query(x0, y0, x1, y1).filter(o => !before.has(o.id) && !['link', 'path', 'arrow'].includes(o.t)).slice(0, 600).map(o => box(o).slice()) : [];
  return { kind: 'move', x0: p.wx, y0: p.wy, before, mb, cands };
}
function shiftTo(o, b0, dx, dy) {
  if (o.t === 'link') { for (const k of ['a', 'b']) if (b0[k] && !b0[k].id) o[k] = { ...b0[k], x: b0[k].x + dx, y: b0[k].y + dy }; return; }
  o.x = b0.x + dx; o.y = b0.y + dy; if (o.t === 'arrow') { o.x2 = b0.x2 + dx; o.y2 = b0.y2 + dy; }
}
/** Умные направляющие: края и центры соседей, равные промежутки. Возвращает поправленные dx, dy. */
function guide(d, dx, dy) {
  const tol = 6 / B.view.z, sh = (b, x, y) => [b[0] + x, b[1] + y, b[2] + x, b[3] + y];
  const X = b => [b[0], (b[0] + b[2]) / 2, b[2]], Y = b => [b[1], (b[1] + b[3]) / 2, b[3]];
  let m = sh(d.mb, dx, dy), bx = null, by = null;
  for (const c of d.cands) {
    for (const a of X(m)) for (const v of X(c)) { const df = v - a; if (Math.abs(df) <= tol && (!bx || Math.abs(df) < Math.abs(bx.df))) bx = { df, v }; }
    for (const a of Y(m)) for (const v of Y(c)) { const df = v - a; if (Math.abs(df) <= tol && (!by || Math.abs(df) < Math.abs(by.df))) by = { df, v }; }
  }
  if (bx) dx += bx.df; if (by) dy += by.df; m = sh(d.mb, dx, dy);
  const near = (vals, v) => vals.some(u => Math.abs(u - v) < 0.5);
  if (bx) { let y0 = m[1], y1 = m[3]; for (const c of d.cands) if (near(X(c), bx.v)) { y0 = Math.min(y0, c[1]); y1 = Math.max(y1, c[3]); } B.guides.push([bx.v, y0, bx.v, y1]); }
  if (by) { let x0 = m[0], x1 = m[2]; for (const c of d.cands) if (near(Y(c), by.v)) { x0 = Math.min(x0, c[0]); x1 = Math.max(x1, c[2]); } B.guides.push([x0, by.v, x1, by.v]); }
  // равные промежутки между соседями слева и справа (сверху и снизу)
  if (!bx) {
    const row = d.cands.filter(c => c[3] > m[1] && c[1] < m[3]);
    const L = row.filter(c => c[2] <= m[0]).sort((a, b) => b[2] - a[2])[0], R = row.filter(c => c[0] >= m[2]).sort((a, b) => a[0] - b[0])[0];
    if (L && R) { const gl = m[0] - L[2], gr = R[0] - m[2]; if (Math.abs(gl - gr) <= tol * 2) { const k = (gr - gl) / 2; dx += k; m = sh(d.mb, dx, dy); const y = (m[1] + m[3]) / 2, g = m[0] - L[2]; B.gaps.push({ x0: L[2], y0: y, x1: m[0], y1: y, len: g }, { x0: m[2], y0: y, x1: R[0], y1: y, len: g }); } }
  }
  if (!by) {
    const col = d.cands.filter(c => c[2] > m[0] && c[0] < m[2]);
    const T = col.filter(c => c[3] <= m[1]).sort((a, b) => b[3] - a[3])[0], D = col.filter(c => c[1] >= m[3]).sort((a, b) => a[1] - b[1])[0];
    if (T && D) { const gt = m[1] - T[3], gd = D[1] - m[3]; if (Math.abs(gt - gd) <= tol * 2) { const k = (gd - gt) / 2; dy += k; m = sh(d.mb, dx, dy); const x = (m[0] + m[2]) / 2, g = m[1] - T[3]; B.gaps.push({ x0: x, y0: T[3], x1: x, y1: m[1], len: g }, { x0: x, y0: m[3], x1: x, y1: D[1], len: g }); } }
  }
  return [dx, dy];
}
function erase(p) {
  const o = hit(p.wx, p.wy, 10); if (!o) return;
  const ids = [o.id, ...linksOf(new Set([o.id]))];
  for (const id of ids) { const x = B.objs.get(id); if (!x) continue; B.drag.before.set(id, copy(x)); delObj(id); }
  send({ t: 'del', ids });
}
function simplify(pts, eps) { // Рамер — Дуглас — Пекер
  const n = pts.length / 2; if (n < 3) return pts; const keep = new Uint8Array(n); keep[0] = keep[n - 1] = 1; const st = [[0, n - 1]];
  while (st.length) { const [a, b] = st.pop(); let md = 0, mi = -1; for (let i = a + 1; i < b; i++) { const d = segDist(pts[2 * i], pts[2 * i + 1], pts[2 * a], pts[2 * a + 1], pts[2 * b], pts[2 * b + 1]); if (d > md) { md = d; mi = i; } } if (md > eps) { keep[mi] = 1; st.push([a, mi], [mi, b]); } }
  const out = []; for (let i = 0; i < n; i++) if (keep[i]) out.push(pts[2 * i], pts[2 * i + 1]); return out;
}
function dbl(e) { const p = pt(e), o = hit(p.wx, p.wy, 4); if (o && hasText(o) && canEdit()) editText(o, false); }
function wheel(e) {
  e.preventDefault(); const p = pt(e); B.follow = '';
  if (e.shiftKey && !e.ctrlKey) { B.view.x += e.deltaY / B.view.z; dirty(); return; }
  const k = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015)); zoomAt(p.sx, p.sy, B.view.z * k);
}
function zoomAt(sx, sy, z) { z = Math.max(0.02, Math.min(8, z)); const [wx, wy] = S2W(sx, sy); B.view.z = z; B.view.x = wx - sx / z; B.view.y = wy - sy / z; dirty(); }
function fit() {
  if (B.W < 10 || B.H < 10) return; const b = worldBounds(); if (!b) { B.view = { x: -B.W / 2, y: -B.H / 2, z: 1 }; dirty(); return; }
  const pad = 60, z = Math.max(0.02, Math.min(2, Math.min(B.W / (b[2] - b[0] + pad * 2), B.H / (b[3] - b[1] + pad * 2))));
  B.view.z = z; B.view.x = (b[0] + b[2]) / 2 - B.W / 2 / z; B.view.y = (b[1] + b[3]) / 2 - B.H / 2 / z; dirty();
}

// текст и заметки: поле поверх доски
function editText(o, fresh) {
  B.edit_el = o.id; B.editBefore = fresh ? null : copy(o); B.sel.clear(); B.hover = null;
  const t = el.text, z = B.view.z, [sx, sy] = W2S(o.x, o.y), fs = (o.fs || 16) * z, top = B.full ? 52 : 0;
  t.classList.remove('hide'); t.value = o.text || '';
  if (o.t === 'shape' || o.t === 'link') { styleEditor(o); placeEditor(o); paintTxt(); dirty(); setTimeout(() => t.focus(), 0); return; }
  Object.assign(t.style, {
    left: sx + (o.t === 'note' ? 14 * z : 0) + 'px', top: top + sy + (o.t === 'note' ? 14 * z : 0) + 'px', fontSize: fs + 'px', lineHeight: o.t === 'note' ? 1.35 : 1.3, color: o.t === 'note' ? '#2E2500' : o.c || '#E8EAFB',
    width: o.t === 'note' ? (o.w - 28) * z + 'px' : Math.max(120, (box(o)[2] - o.x) * z + fs * 4) + 'px', height: Math.max(fs * 1.6, (box(o)[3] - o.y - (o.t === 'note' ? 28 : 0)) * z) + 'px', background: o.t === 'note' ? 'transparent' : 'rgba(14,15,34,.6)',
  });
  styleEditor(o);
  paintTxt();
  dirty(); setTimeout(() => t.focus(), 0);
}
/** Поле текста фигуры (по центру её текста) и подписи связи (на линии). */
function placeEditor(o) {
  const t = el.text, z = B.view.z, top = B.full ? 52 : 0, fs = (o.fs || (o.t === 'link' ? 15 : 18)) * z;
  let tb;
  if (o.t === 'shape') { tb = DG.textBox(o); if (o.k === 'lane') tb = [o.x + 6, o.y + 6, Math.max(180, o.w * 0.3), o.h - 12]; }
  else { const c = B.cache.get(o.id) || (box(o), B.cache.get(o.id)); const [px, py] = c && c.g ? DG.along(c.g.poly, o.lt ?? 0.5) : [o.x || 0, o.y || 0]; const w = Math.max(180 / z, (c && c.lb ? c.lb[2] - c.lb[0] : 0) + 40 / z); tb = [px - w / 2, py - fs / z, w, (fs * 2) / z]; }
  const [sx, sy] = W2S(tb[0], tb[1]), w = Math.max(60, tb[2] * z);
  Object.assign(t.style, { left: sx + 'px', width: w + 'px', lineHeight: 1.3, padding: '0', background: o.t === 'link' ? bgColor() : 'rgba(14,15,34,.25)' });
  t.style.height = '0px'; const h = Math.max(fs * 1.4, t.scrollHeight); t.style.height = h + 'px';
  const va = o.t === 'link' || o.k === 'frame' ? 'middle' : o.va || 'middle', H = tb[3] * z;
  t.style.top = top + (va === 'top' ? sy + 4 * z : va === 'bottom' ? sy + H - h : sy + (H - h) / 2) + 'px';
}
function textInput() { const o = B.objs.get(B.edit_el); if (!o) return; o.text = el.text.value; setObj(o); send({ t: 'put', o }); if (o.t === 'shape' || o.t === 'link') { placeEditor(o); return; } el.text.style.height = 'auto'; el.text.style.height = el.text.scrollHeight + 'px'; if (o.t === 'text') el.text.style.width = Math.max(120, (box(o)[2] - o.x) * B.view.z + 40) + 'px'; }
function styleEditor(o) {
  const t = el.text; if (!t) return;
  const dg = o.t === 'shape' || o.t === 'link';
  Object.assign(t.style, { fontFamily: "'" + famOf(o) + "', Manrope, sans-serif", fontWeight: o.fb ? '700' : '500', fontStyle: o.fi ? 'italic' : 'normal', textDecoration: o.fu ? 'underline' : 'none', textAlign: o.fa || (dg && o.k !== 'frame' ? 'center' : 'left'),
    color: o.t === 'note' ? (o.c && o.c !== '#E8EAFB' ? o.c : '#2E2500') : o.t === 'shape' ? shapeText(o) : o.t === 'link' ? o.tc || o.c || '#E8EAFB' : o.c || '#E8EAFB', fontSize: (o.fs || (o.t === 'link' ? 15 : o.t === 'shape' ? 18 : 16)) * B.view.z + 'px' });
  if (dg && B.edit_el === o.id) placeEditor(o);
}
function textDone() {
  const id = B.edit_el; if (!id) return; B.edit_el = null; el.text.classList.add('hide'); paintTxt(); const o = B.objs.get(id); if (!o) return;
  if (o.t === 'shape' || o.t === 'link') { // пустой текст фигуры — просто без текста
    if (!String(o.text || '').trim()) { delete o.text; setObj(o); }
    send({ t: 'put', o }); if (JSON.stringify(B.editBefore) !== JSON.stringify(copy(o))) hist(new Map([[id, B.editBefore]]), new Map([[id, copy(o)]])); dirty(); return;
  }
  if (!String(o.text || '').trim()) { delObj(id); send({ t: 'del', ids: [id] }); if (B.editBefore) hist(new Map([[id, B.editBefore]]), new Map([[id, null]])); return; }
  send({ t: 'put', o }); hist(new Map([[id, B.editBefore]]), new Map([[id, copy(o)]])); dirty();
}

// картинки и PDF: файл, вставка, перетаскивание; картинки уменьшаются до «Картинки: до N px»
function pickFiles(x, y) { const inp = document.createElement('input'); inp.type = 'file'; inp.accept = 'image/*,application/pdf,.pdf'; inp.multiple = true; inp.onchange = () => addFiles([...inp.files], x, y); inp.click(); }
async function shrink(f) {
  const max = +SET.bImg || 0; if (!max || f.type === 'image/gif') return f;
  try {
    const img = await createImageBitmap(f); if (Math.max(img.width, img.height) <= max) return f;
    const k = max / Math.max(img.width, img.height), c = document.createElement('canvas'); c.width = Math.round(img.width * k); c.height = Math.round(img.height * k); c.getContext('2d').drawImage(img, 0, 0, c.width, c.height);
    return await new Promise(r => c.toBlob(r, f.type === 'image/png' ? 'image/png' : 'image/jpeg', 0.9));
  } catch { return f; }
}
const isPdf = f => f.type === 'application/pdf' || /\.pdf$/i.test(f.name);
async function addFiles(files, x, y) {
  if (!canEdit()) { toast(B.enabled ? 'Доску правит только владелец канала' : 'Доска выключена', 'bad'); return; }
  if (layerHiddenWarn()) return;
  let dx = 0;
  for (const f of files) {
    if (isPdf(f)) { dx += await addPdf(f, x + dx, y); continue; }
    if (!/^image\//.test(f.type)) continue;
    try {
      const blob = await shrink(f); const bmp = await createImageBitmap(blob); const r = await api('POST', 'board/image?room=' + encodeURIComponent(B.room), blob, true);
      const k = Math.min(1, 600 / Math.max(bmp.width, bmp.height)) / Math.max(0.02, Math.min(4, B.view.z));
      const o = { id: newId(), t: 'img', L: B.layer, src: r.id, x: Math.round(x + dx), y: Math.round(y), w: Math.round(bmp.width * k), h: Math.round(bmp.height * k), z: Date.now() };
      dx += o.w + 20 / B.view.z; change(new Map([[o.id, null]]), new Map([[o.id, o]]));
    } catch (e) { toast('Картинка «' + f.name + '»: ' + e.message, 'bad'); }
  }
}
/** PDF: каждая страница становится картинкой на доске (столбиком). Соседям pdf.js не нужен — они видят картинки. */
async function pdfLib() {
  const pdfjs = await import('pdfjs-dist');
  const worker = (await import('pdfjs-dist/build/pdf.worker.min.mjs?url')).default;
  pdfjs.GlobalWorkerOptions.workerSrc = worker;
  return pdfjs;
}
/** Страница PDF → JPEG (длинная сторона до target px) и её размер в пунктах. */
async function pdfPage(doc, i, target) {
  const page = await doc.getPage(i);
  const v1 = page.getViewport({ scale: 1 });
  const vp = page.getViewport({ scale: Math.min(3, target / Math.max(v1.width, v1.height)) });
  const c = document.createElement('canvas'); c.width = Math.round(vp.width); c.height = Math.round(vp.height);
  const ctx = c.getContext('2d'); ctx.fillStyle = '#fff'; ctx.fillRect(0, 0, c.width, c.height);
  await page.render({ canvasContext: ctx, viewport: vp, canvas: c }).promise;
  const blob = await new Promise(r => c.toBlob(r, 'image/jpeg', 0.88));
  return { blob, w: v1.width, h: v1.height };
}
async function addPdf(f, x, y) {
  let pdfjs;
  try { pdfjs = await pdfLib(); } catch (e) { toast('PDF: не удалось загрузить просмотрщик — ' + e.message, 'bad'); return 0; }
  let doc;
  try { doc = await pdfjs.getDocument({ data: await f.arrayBuffer() }).promise; } catch (e) { toast('PDF «' + f.name + '»: ' + e.message, 'bad'); return 0; }
  let pages = doc.numPages;
  if (pages > 40 && !confirm('В «' + f.name + '» ' + pages + ' страниц. Положить на доску первые 40?\n(Отмена — все ' + pages + '; это займёт время и место.)')) pages = doc.numPages; else if (pages > 40) pages = 40;
  const name = f.name.replace(/\.pdf$/i, '');
  const zk = 1 / Math.max(0.02, Math.min(4, B.view.z));
  let cy = y, maxW = 0;
  const before = new Map(), after = new Map();
  toast('PDF «' + name + '»: кладу ' + pages + ' стр. на доску…');
  for (let i = 1; i <= pages; i++) {
    try {
      const { blob, w: pw, h: ph } = await pdfPage(doc, i, Math.min(+SET.bImg || 2560, 1800));
      const v1 = { width: pw, height: ph };
      const r = await api('POST', 'board/image?room=' + encodeURIComponent(B.room), blob, true);
      const w = Math.round(Math.min(1, 700 / v1.width) * v1.width * zk), h = Math.round((w * v1.height) / v1.width);
      const o = { id: newId(), t: 'img', L: B.layer, src: r.id, x: Math.round(x), y: Math.round(cy), w, h, z: Date.now() + i, pdf: name.slice(0, 60), page: i };
      setObj(o); send({ t: 'put', o }); before.set(o.id, null); after.set(o.id, o);
      cy += h + 40 * zk; maxW = Math.max(maxW, w);
    } catch (e) { toast('PDF стр. ' + i + ': ' + e.message, 'bad'); }
  }
  if (after.size) hist(before, after);
  try { doc.destroy(); } catch {}
  return maxW + 40 * zk;
}
// ── копирование объектов: Ctrl+C / Ctrl+X / Ctrl+V, Ctrl+D — дубликат ──
// В буфер кладётся JSON с меткой most-board: выделенное, связи между ними (связь, у которой второй конец вне
// выделения, становится свободной), а рамки и дорожки — вместе с тем, что на них лежит. Вставка — с новыми id.
const CLIP_MARK = 'most-board';
function clipPack() {
  if (!B.sel.size) return null;
  const ids = new Set([...B.sel].filter(id => B.objs.has(id)));
  for (const id of [...ids]) {
    const c = B.objs.get(id); if (!DG.isContainer(c)) continue;
    for (const o of query(c.x, c.y, c.x + c.w, c.y + c.h, true)) { if (ids.has(o.id) || layerOf(o) !== layerOf(c) || o.t === 'link') continue; const b = box(o); if (b[0] >= c.x - 1 && b[1] >= c.y - 1 && b[2] <= c.x + c.w + 1 && b[3] <= c.y + c.h + 1) ids.add(o.id); }
  }
  for (const lid of linksOf(ids)) { const l = B.objs.get(lid); if (l && ends(l).every(x => ids.has(x))) ids.add(lid); }
  const objs = [];
  for (const id of ids) {
    const o = copy(B.objs.get(id)); if (!o) continue;
    if (o.t === 'link') {
      const poly = (B.cache.get(id) || (box(B.objs.get(id)), B.cache.get(id)) || {}).g?.poly || [];
      for (const [k, pi] of [['a', 0], ['b', poly.length - 1]] as [string, number][]) if (o[k] && o[k].id && !ids.has(o[k].id)) { const pt = poly[pi]; o[k] = pt ? { x: pt[0], y: pt[1] } : { x: 0, y: 0 }; }
    }
    objs.push(o);
  }
  if (!objs.length) return null;
  return { [CLIP_MARK]: 1, room: B.room, objs };
}
function clipBox(objs) {
  let b = null;
  for (const o of objs) { if (o.t === 'link') { for (const k of ['a', 'b']) if (o[k] && !o[k].id) { const x = o[k].x, y = o[k].y; b = b ? [Math.min(b[0], x), Math.min(b[1], y), Math.max(b[2], x), Math.max(b[3], y)] : [x, y, x, y]; } continue; }
    const x = box(o); b = b ? [Math.min(b[0], x[0]), Math.min(b[1], x[1]), Math.max(b[2], x[2]), Math.max(b[3], x[3])] : x.slice(); }
  return b || [0, 0, 0, 0];
}
let pasteN = 0, lastPasteSig = '';
async function clipPaste(pk, at) {
  if (!canEdit() || !pk || !Array.isArray(pk.objs) || !pk.objs.length) return;
  const src = pk.objs.slice(0, 3000), map = new Map(), gmap = new Map();
  for (const o of src) if (o && o.id) map.set(o.id, newId());
  const b = clipBox(src), sig = JSON.stringify(src.map(o => o.id));
  pasteN = sig === lastPasteSig ? pasteN + 1 : 1; lastPasteSig = sig;
  let dx, dy;
  if (at) { dx = at[0] - (b[0] + b[2]) / 2; dy = at[1] - (b[1] + b[3]) / 2; } else { dx = dy = 30 * pasteN; }
  if (SET.bSnap) { dx = Math.round(dx / 10) * 10; dy = Math.round(dy / 10) * 10; } else { dx = Math.round(dx); dy = Math.round(dy); }
  const before = new Map(), after = new Map(), z0 = Date.now(), other = pk.room && pk.room !== B.room;
  let zi = 0;
  for (const o0 of [...src].sort((a, b2) => (a.z || 0) - (b2.z || 0))) {
    if (!o0 || !map.has(o0.id)) continue;
    const o = JSON.parse(JSON.stringify(o0)); o.id = map.get(o0.id); o.z = z0 + zi++; o.L = B.layer;
    if (o.g) { if (!gmap.has(o.g)) gmap.set(o.g, newId()); o.g = gmap.get(o.g); }
    if (o.t === 'link') { for (const k of ['a', 'b']) { if (!o[k]) continue; if (o[k].id) { if (map.has(o[k].id)) o[k] = { ...o[k], id: map.get(o[k].id) }; else o[k] = { x: 0, y: 0 }; } else o[k] = { ...o[k], x: o[k].x + dx, y: o[k].y + dy }; } }
    else shiftTo(o, o0, dx, dy);
    if (o.t === 'img' && other && o.src) { // картинка из другой доски — перенести файл сюда
      try { const r0 = await fetch(aurl('board/image?room=' + encodeURIComponent(pk.room) + '&id=' + encodeURIComponent(o.src))); if (r0.ok) { const r = await api('POST', 'board/image?room=' + encodeURIComponent(B.room), await r0.blob(), true); o.src = r.id; } } catch {}
    }
    before.set(o.id, null); after.set(o.id, o);
  }
  // сначала фигуры, потом связи — чтобы концы нашлись
  const ord = new Map([...after].sort((x, y) => (x[1].t === 'link' ? 1 : 0) - (y[1].t === 'link' ? 1 : 0)));
  change(before, ord);
  B.sel = new Set([...after.keys()]); setTool('select'); dirty();
  toast('Вставлено: ' + after.size, 'ok');
}
function clipText(e) { try { const t = e.clipboardData?.getData('text/plain') || ''; if (!t.includes(CLIP_MARK)) return null; const v = JSON.parse(t); return v && v[CLIP_MARK] ? v : null; } catch { return null; } }
const boardFocus = e => B.vis && !e.target.matches('input,textarea,select') && !e.target.isContentEditable && !popOpen() && !document.querySelector('.streams [data-stream]:hover');
document.addEventListener('copy', e => { if (!boardFocus(e) || String(getSelection() || '')) return; const pk = clipPack(); if (!pk) return; e.preventDefault(); B.clip = pk; e.clipboardData.setData('text/plain', JSON.stringify(pk)); pasteN = 0; lastPasteSig = JSON.stringify(pk.objs.map(o => o.id)); toast('Скопировано: ' + pk.objs.length, 'ok'); });
document.addEventListener('cut', e => {
  if (!boardFocus(e) || String(getSelection() || '') || !canEdit()) return; const pk = clipPack(); if (!pk) return;
  e.preventDefault(); B.clip = pk; e.clipboardData.setData('text/plain', JSON.stringify(pk)); pasteN = 0; lastPasteSig = '';
  const ids = new Set(pk.objs.map(o => o.id).filter(id => B.objs.has(id))); for (const l of linksOf(ids)) ids.add(l);
  change(new Map([...ids].map(id => [id, copy(B.objs.get(id))])), new Map([...ids].map(id => [id, null]))); B.sel.clear();
});
document.addEventListener('paste', e => {
  if (!B.vis || e.target.matches('input,textarea')) return;
  const pk = clipText(e);
  if (pk) { e.preventDefault(); clipPaste(pk, B.mouse.in ? [B.mouse.wx, B.mouse.wy] : null); return; }
  const files = [...(e.clipboardData?.files || [])].filter(f => /^image\//.test(f.type) || isPdf(f)); if (!files.length) return;
  e.preventDefault(); const [x, y] = B.mouse.in ? [B.mouse.wx, B.mouse.wy] : S2W(B.W / 2, B.H / 2); addFiles(files, x, y);
});
document.addEventListener('keydown', e => {
  if (!B.vis || e.target.matches('input,textarea,select') || popOpen()) return;
  if (e.code === 'Space') { if (!B.space) { B.space = true; el.cv.style.cursor = 'grab'; } e.preventDefault(); return; }
  const k = e.key.toLowerCase(), mod = e.ctrlKey || e.metaKey;
  if (mod && (k === 'z' || k === 'я')) { e.preventDefault(); e.shiftKey ? redo() : undo(); return; }
  if (mod && (k === 'y' || k === 'н')) { e.preventDefault(); redo(); return; }
  if (mod && k === '0') { e.preventDefault(); fit(); return; }
  if (mod && (k === 'a' || k === 'ф')) { e.preventDefault(); const [x0, y0] = S2W(0, 0), [x1, y1] = S2W(B.W, B.H); for (const o of query(x0, y0, x1, y1)) if (inActive(o)) B.sel.add(o.id); setTool('select'); dirty(); return; }
  if ((e.key === 'Delete' || e.key === 'Backspace') && B.sel.size && canEdit()) { e.preventDefault(); const ids = new Set([...B.sel, ...linksOf(B.sel)]); const before = new Map([...ids].map(id => [id, copy(B.objs.get(id))])); change(before, new Map([...ids].map(id => [id, null]))); B.sel.clear(); return; }
  if (e.key === 'Tab' && B.sel.size === 1 && canEdit()) { const o = B.objs.get([...B.sel][0]); if (o && o.t === 'shape' && !DG.isContainer(o)) { e.preventDefault(); quickAdd(o, e.shiftKey ? { r: 'l', l: 'r', t: 'b', b: 't' }[B.lastSide || 'r'] : B.lastSide || 'r'); return; } }
  if (mod && (k === 'd' || k === 'в') && B.sel.size && canEdit()) { e.preventDefault(); const pk = clipPack(); if (pk) { lastPasteSig = ''; pasteN = 0; clipPaste(pk, null); } return; }
  if (mod && (k === 'g' || k === 'п')) { e.preventDefault(); arrange(e.shiftKey ? 'ungroup' : 'group'); return; }
  if (e.key === 'Escape') { if (B.full) { full(false); return; } B.sel.clear(); dirty(); return; }
  if (e.key === '+' || e.key === '=') { zoomAt(B.W / 2, B.H / 2, B.view.z * 1.25); return; }
  if (e.key === '-') { zoomAt(B.W / 2, B.H / 2, B.view.z / 1.25); return; }
  if (mod || e.altKey) return;
  if (document.querySelector('.streams [data-stream]:hover')) return; // мышь над показом экрана — клавиши его инструментов
  const keys = { b: 'brush', и: 'brush', v: 'select', м: 'select', h: 'hand', р: 'hand', p: 'pen', з: 'pen', m: 'marker', ь: 'marker', e: 'eraser', у: 'eraser', r: 'rect', к: 'rect', a: 'arrow', ф: 'arrow', t: 'text', е: 'text', n: 'note', т: 'note', i: 'image', ш: 'image', l: 'link', д: 'link', s: 'shape', ы: 'shape' };
  if ((k === 'l' || k === 'д') && e.shiftKey) { toggleLayers(); return; }
  if (keys[k]) {
    if (keys[k] === 'image') { const [x, y] = S2W(B.W / 2 - 150, B.H / 2 - 100); pickFiles(x, y); }
    else if (keys[k] === 'shape' && B.tool === 'shape') shapePicker(el.host.querySelector('[data-tool="shape"]'));
    else if (keys[k] === 'layers') toggleLayers();
    else setTool(keys[k]);
  }
});
document.addEventListener('keyup', e => { if (e.code === 'Space' && B.space) { B.space = false; if (el.cv) el.cv.style.cursor = ''; } });

// ───────────── кнопки ─────────────
function setTool(t) { B.tool = t; if (t !== 'select') B.sel.clear(); paintTools(); dirty(); }
function paintTools() {
  if (!el.host) return;
  el.host.querySelectorAll('[data-tool]').forEach(b => { b.classList.toggle('on', b.dataset.tool === B.tool); b.disabled = !canEdit() && !['select', 'hand'].includes(b.dataset.tool); });
  el.host.querySelectorAll('[data-color]').forEach(b => b.classList.toggle('on', b.dataset.color === B.color));
  el.host.querySelectorAll('[data-size]').forEach(b => b.classList.toggle('on', +b.dataset.size === B.size));
  el.host.querySelector('.brr').classList.toggle('hide', B.tool !== 'brush'); el.host.querySelector('.szs').classList.toggle('hide', B.tool === 'brush');
  const lb = el.host.querySelector('[data-bd="layers"]'); if (lb) lb.classList.toggle('on', B.layersOpen);
  el.lay.textContent = 'слой: ' + (activeLayer()?.name || '—') + (B.hidden.has(B.layer) ? ' (скрыт)' : '');
  el.cv.dataset.cur = B.tool;
  const sb = el.host.querySelector('[data-tool="shape"]'); if (sb && sb.dataset.k !== B.shapeK) { sb.dataset.k = B.shapeK; sb.innerHTML = shapeIcon(B.shapeK, 20); }
  paintTxt();
}
function click(e) {
  const t = e.target.closest('[data-tool]'); if (t) { if (t.dataset.tool === 'image') { const [x, y] = S2W(B.W / 2 - 150, B.H / 2 - 100); pickFiles(x, y); return; } setTool(t.dataset.tool); if (t.dataset.tool === 'shape') shapePicker(t); return; }
  const c = e.target.closest('[data-color]'); if (c) { B.color = c.dataset.color; if (['select', 'hand', 'eraser'].includes(B.tool)) setTool('pen'); recolorSel(); paintTools(); return; }
  const s = e.target.closest('[data-size]'); if (s) { B.size = +s.dataset.size; if (['select', 'hand', 'eraser'].includes(B.tool)) setTool('pen'); paintTools(); return; }
  const em = e.target.closest('[data-em]'); if (em) { if (B.owner !== ME()) return; send({ t: 'edit', mode: em.dataset.em }); B.edit = em.dataset.em; ui(); return; }
  const bgb = e.target.closest('[data-bg]'); if (bgb) { setBg({ c: bgb.dataset.bg }); return; }
  const ly = e.target.closest('[data-ly]'); if (ly) { layerAction(ly.dataset.ly, ly.dataset.id, e); return; }
  const b = e.target.closest('[data-bd]'); if (!b) return;
  const a = b.dataset.bd;
  if (a === 'undo') undo(); else if (a === 'redo') redo();
  else if (a === 'zin') zoomAt(B.W / 2, B.H / 2, B.view.z * 1.25); else if (a === 'zout') zoomAt(B.W / 2, B.H / 2, B.view.z / 1.25); else if (a === 'fit') fit();
  else if (a === 'full') full(!B.full);
  else if (a === 'cfg') { el.cfg.classList.toggle('hide'); if (!el.cfg.classList.contains('hide')) { B.layersOpen = false; el.layers.classList.add('hide'); } fillFollow(); bindSets(el.cfg, () => dirty()); paintBg(); stat(); paintTools(); }
  else if (a === 'layers') toggleLayers();
  else if (a === 'on' || a === 'off') { const on = a === 'on'; send({ t: 'enable', on }); B.enabled = on; ui(); if (on) toast('Доска включена — видят все в канале', 'ok'); }
  else if (a === 'export') exportPop(b);
  else if (a === 'stands') standsPop(b);
}
function recolorSel() {
  if (!B.sel.size || !canEdit()) return; const before = new Map(), after = new Map();
  for (const id of B.sel) { const o = B.objs.get(id); if (!o || o.t === 'img') continue; before.set(id, copy(o)); const n = copy(o); if (o.t === 'note') n.bg = NOTE_BG[B.color] || n.bg; else n.c = B.color; after.set(id, n); }
  if (after.size) change(before, after);
}
function fillFollow() { const cur = el.follow.value; el.follow.innerHTML = '<option value="">ни за кем</option>' + [...B.curs.keys()].map(n => '<option value="' + esc(n) + '">' + esc(titleOf(n)) + '</option>').join(''); el.follow.value = B.follow || cur || ''; }
function full(on) { B.full = on; el.bd.classList.toggle('full', on); document.body.classList.toggle('bdfull', on); el.title.textContent = R.title || ''; setTimeout(resize, 0); }
function ui() {
  if (!el.host) return;
  const ro = B.enabled && !canEdit();
  el.off.classList.toggle('hide', B.enabled || !B.loaded);
  el.dot.style.background = B.err ? 'var(--amber)' : B.enabled ? 'var(--green)' : 'var(--dim)';
  el.st.textContent = B.err ? 'Нет связи с доской: ' + B.err : !B.loaded ? 'Загружаю доску…' : !B.enabled ? 'Доска выключена' : ro ? 'Только смотреть — правит владелец канала' : 'Доска включена · рисуют все в канале';
  el.host.querySelector('[data-bd="off"]').disabled = !B.enabled;
  el.host.querySelectorAll('[data-em]').forEach(b => { b.classList.toggle('on', b.dataset.em === B.edit); b.disabled = B.owner !== ME(); });
  el.host.querySelector('.bdeditnote').textContent = B.owner === ME() ? 'Вы владелец канала — решаете, кто правит.' : 'Решает владелец канала: ' + titleOf(B.owner);
  emit();
  paintTools(); paintLayers(); dirty();
}

// ───────────── текст: шрифт, размер, стиль ─────────────
const styleOf = d => { const st: any = {}; if (d.ff) st.ff = d.ff; if (d.fb) st.fb = true; if (d.fi) st.fi = true; if (d.fu) st.fu = true; if (d.fa && d.fa !== 'left') st.fa = d.fa; return st; };
function txtDef() { return { ff: '', fb: false, fi: false, fu: false, fa: '', fs: 22, ...(SET.bTxt || {}) }; }
/** К каким объектам относится панель текста: редактируемый, иначе выделенные тексты и заметки. */
function txtTargets() {
  if (B.edit_el) { const o = B.objs.get(B.edit_el); return o ? [o] : []; }
  return [...B.sel].map(id => B.objs.get(id)).filter(hasText);
}
function txtContext() { return !!B.edit_el || B.tool === 'text' || B.tool === 'note' || txtTargets().length > 0; }
function paintTxt() {
  if (!el.txt) return;
  const on = txtContext() && canEdit(); el.txt.classList.toggle('hide', !on); if (!on) return;
  const t = txtTargets()[0], dgDef = t && (t.t === 'link' || (t.t === 'shape' && t.k !== 'frame')) ? 'center' : '';
  const d = t ? { ff: t.ff || '', fb: !!t.fb, fi: !!t.fi, fu: !!t.fu, fa: t.fa || dgDef, fs: t.fs || 20 } : txtDef();
  el.txt.querySelectorAll('[data-ff]').forEach(b => b.classList.toggle('on', b.dataset.ff === (d.ff || '')));
  for (const k of ['fb', 'fi', 'fu']) el.txt.querySelector('[data-tx="' + k + '"]').classList.toggle('on', !!d[k]);
  for (const a of ['left', 'center', 'right']) el.txt.querySelector('[data-tx="' + a + '"]').classList.toggle('on', (d.fa || 'left') === a);
  const sh = txtTargets().filter(o => o.t === 'shape' && !DG.isContainer(o)); el.txt.querySelector('.txva').classList.toggle('hide', !sh.length);
  if (sh.length) for (const v of ['top', 'mid', 'bot']) el.txt.querySelector('[data-tx="v' + v + '"]').classList.toggle('on', ({ top: 'top', bot: 'bottom' }[v] || 'middle') === (sh[0].va || 'middle'));
  if (document.activeElement !== el.txtfs) el.txtfs.value = String(Math.round(d.fs));
  el.txtfs.title = t ? 'Размер текста (в единицах доски)' : 'Размер нового текста (как на экране сейчас)';
}
function txtClick(e) {
  const f = e.target.closest('[data-ff]'); if (f) { txtSet('ff', f.dataset.ff); return; }
  const b = e.target.closest('[data-tx]'); if (!b) return; const a = b.dataset.tx;
  if (a === 'up' || a === 'dn') {
    const t = txtTargets()[0], cur = t ? t.fs || 20 : txtDef().fs;
    const v = a === 'up' ? FS_STEPS.find(x => x > cur + 0.5) || Math.round(cur * 1.25) : [...FS_STEPS].reverse().find(x => x < cur - 0.5) || Math.max(4, Math.round(cur / 1.25));
    txtSet('fs', v); return;
  }
  if (a === 'left' || a === 'center' || a === 'right') { const t0 = txtTargets()[0], dg = t0 && (t0.t === 'link' || (t0.t === 'shape' && t0.k !== 'frame')); txtSet('fa', dg ? (a === 'center' ? '' : a) : a === 'left' ? '' : a); return; }
  if (a === 'vtop' || a === 'vmid' || a === 'vbot') { applyTo(txtTargets().filter(o => o.t === 'shape'), n => { if (a === 'vmid') delete n.va; else n.va = a === 'vtop' ? 'top' : 'bottom'; }); if (B.edit_el) { const o = B.objs.get(B.edit_el); if (o) placeEditor(o); setTimeout(() => el.text.focus(), 0); } paintTxt(); return; }
  txtSet(a, null);
}
/** Поменять стиль: у редактируемого/выделенных текстов (с отменой) и как стиль по умолчанию для новых. v === null — переключить. */
function txtSet(k, v) {
  const list = txtTargets();
  const first = list[0], cur = first ? first[k] : txtDef()[k];
  if (v === null) v = !cur;
  const d = txtDef(); if (k !== 'fs' || !list.length) { d[k] = v; setSet('bTxt', d); }
  if (list.length && canEdit()) {
    const before = new Map(), after = new Map();
    for (const o of list) {
      const n = copy(o);
      if (v === false || v === '') delete n[k]; else n[k] = v;
      if (B.edit_el === o.id) { Object.assign(o, n); for (const kk of ['ff', 'fb', 'fi', 'fu', 'fa']) if (!(kk in n)) delete o[kk]; setObj(o); send({ t: 'put', o }); styleEditor(o); textInput(); continue; }
      before.set(o.id, copy(o)); after.set(o.id, n);
    }
    if (after.size) change(before, after);
  }
  paintTxt(); dirty();
  if (B.edit_el) setTimeout(() => el.text.focus(), 0);
}

// ───────────── диаграммы: полоса свойств, меню фигур, компоновка ─────────────
function dgCx() {
  const sel = [...B.sel].map(id => B.objs.get(id)).filter(Boolean);
  const shapes = sel.filter(o => o.t === 'shape'), links = sel.filter(o => o.t === 'link'), boxes = sel.filter(o => o.t !== 'link');
  return { shapes, links, many: boxes.length >= 2, groups: boxes.some(o => o.g), newShape: !sel.length && B.tool === 'shape', newLink: !sel.length && B.tool === 'link' };
}
function dgSig() {
  if (!el.dg) return ''; const cx = dgCx(), f = o => o ? [o.k, o.f, o.sd, o.sw, o.ha, o.hb, o.g].join(',') : '';
  return [B.tool, B.edit_el ? 'e' : '', canEdit(), [...B.sel].length, f(cx.shapes[0]), f(cx.links[0]), cx.many, cx.groups, JSON.stringify(SET.bShapeSt || {}), JSON.stringify(SET.bLinkSt || {}), B.shapeK].join('|');
}
const SEP = '<i class="bdsep v"></i>';
const DASH_IC = { '': '<path d="M3 12h18"/>', dash: '<path d="M3 12h4M10 12h4M17 12h4"/>', dot: '<path d="M4 12h.01M9 12h.01M14 12h.01M19 12h.01" stroke-width="3"/>' };
const LK_IC = { straight: '<path d="M5 19L19 5"/>', elbow: '<path d="M5 19v-7h14V5"/>', curve: '<path d="M5 19C5 9 19 15 19 5"/>' };
const LK_NAME = { straight: 'Прямая', elbow: 'Ломаная — под прямым углом', curve: 'Кривая' };
const nearW = sw => { const v = (sw || 2) * kzoom(); return [1, 2, 4].reduce((a, b) => (Math.abs(b - v) < Math.abs(a - v) ? b : a), 2); };
function headIcon(kind) {
  const key = 'h' + kind; if (ICONS.has(key)) return ICONS.get(key);
  const cv = document.createElement('canvas'), d = 2; cv.width = 34 * d; cv.height = 16 * d; const ctx = cv.getContext('2d'); ctx.scale(d, d);
  ctx.strokeStyle = '#E8EAFB'; ctx.lineWidth = 1.6; ctx.lineCap = 'round'; const ins = DG.headInset(kind, 1.6);
  ctx.beginPath(); ctx.moveTo(3, 8); ctx.lineTo(30 - ins, 8); ctx.stroke(); DG.drawHead(ctx, kind, 30, 8, 0, 1.6, '#E8EAFB', '#141730');
  const html = '<img class="hdic" alt="" src="' + cv.toDataURL() + '" width="34" height="16">'; ICONS.set(key, html); return html;
}
function paintDg() {
  if (!el.dg) return;
  const cx = dgCx(), on = canEdit() && !B.edit_el && (cx.shapes.length || cx.links.length || cx.many || cx.newShape || cx.newLink);
  el.dg.classList.toggle('hide', !on); if (!on) return;
  const btn = (a, v, title, inner, isOn) => '<button class="tool' + (isOn ? ' on' : '') + '" data-dg="' + a + '" data-v="' + v + '" title="' + title + '">' + inner + '</button>';
  let h = '';
  if (cx.shapes.length || cx.newShape) {
    const o = cx.shapes[0] || shapeDef();
    h += '<button class="b sm dgk" data-dg="kind" title="' + (cx.shapes.length ? 'Сменить фигуру — текст и связи останутся' : 'Какую фигуру ставить') + '">' + shapeIcon(o.k, 18) + '<span>▾</span></button>' + SEP;
    h += '<span class="dgl">Заливка</span>' + FILLS.map(f => '<button class="sw fsw' + ((o.f || '') === f ? ' on' : '') + '" data-dg="fill" data-v="' + f + '" title="' + (f || 'Без заливки') + '" style="background:' + (f || 'transparent') + '">' + (f ? '' : '<i></i>') + '</button>').join('') + SEP;
    h += '<span class="dgl">Контур</span>' + ['', 'dash', 'dot'].map(v => btn('sd', v, { '': 'Сплошной', dash: 'Пунктир', dot: 'Точки' }[v], SVG(DASH_IC[v]), (o.sd || '') === v)).join('') + [1, 2, 4].map(v => btn('sw', v, 'Толщина ' + v, '<i class="dgw" style="height:' + v + 'px"></i>', nearW(o.sw) === v)).join('');
  }
  if (cx.links.length || cx.newLink) {
    const o = cx.links[0] || linkDef();
    if (h) h += SEP;
    h += ['straight', 'elbow', 'curve'].map(v => btn('lk', v, LK_NAME[v], SVG(LK_IC[v]), (o.k || 'straight') === v)).join('') + SEP;
    h += '<button class="b sm dgh" data-dg="ha" title="Конец у начала связи">' + headIcon(o.ha || 'none').replace('class="hdic"', 'class="hdic flip"') + '</button>' + (cx.links.length ? btn('swap', '', 'Развернуть связь', SVG('<path d="M7 7h11l-3-3M17 17H6l3 3"/>'), false) : '') + '<button class="b sm dgh" data-dg="hb" title="Конец у конца связи">' + headIcon(o.hb === undefined ? 'arrow' : o.hb) + '</button>' + SEP;
    h += ['', 'dash', 'dot'].map(v => btn('lsd', v, { '': 'Сплошная', dash: 'Пунктир — «необязательно»', dot: 'Точки — «зависит»' }[v], SVG(DASH_IC[v]), (o.sd || '') === v)).join('') + [1, 2, 4].map(v => btn('lsw', v, 'Толщина ' + v, '<i class="dgw" style="height:' + v + 'px"></i>', nearW(o.sw) === v)).join('');
  }
  if (cx.many) {
    if (h) h += SEP;
    const A = [['l', 'По левому краю', '<path d="M4 3v18M8 7h10M8 15h6"/>'], ['c', 'По центру', '<path d="M12 3v18M6 7h12M8 15h8"/>'], ['r', 'По правому краю', '<path d="M20 3v18M6 7h10M10 15h6"/>'], ['t', 'По верху', '<path d="M3 4h18M7 8v10M15 8v6"/>'], ['m', 'По середине', '<path d="M3 12h18M7 6v12M15 8v8"/>'], ['b', 'По низу', '<path d="M3 20h18M7 6v10M15 10v6"/>']];
    h += A.map(([v, t, ic]) => btn('al', v, 'Выровнять: ' + t.toLowerCase(), SVG(ic), false)).join('') + SEP;
    h += btn('dist', 'h', 'Распределить по горизонтали', SVG('<path d="M4 4v16M20 4v16M9 8h6v8H9z"/>'), false) + btn('dist', 'v', 'Распределить по вертикали', SVG('<path d="M4 4h16M4 20h16M8 9h8v6H8z"/>'), false);
    h += btn('size', 'w', 'Одинаковая ширина', SVG('<path d="M4 12h16M7 9l-3 3 3 3M17 9l3 3-3 3"/>'), false) + btn('size', 'h', 'Одинаковая высота', SVG('<path d="M12 4v16M9 7l3-3 3 3M9 17l3 3 3-3"/>'), false) + SEP;
    h += btn('group', '', 'Сгруппировать (Ctrl+G)', SVG('<rect x="3" y="3" width="8" height="8" rx="1"/><rect x="13" y="13" width="8" height="8" rx="1"/><path d="M11 7h4v4M13 17H9v-4"/>'), false);
    if (cx.groups) h += btn('ungroup', '', 'Разгруппировать (Ctrl+Shift+G)', SVG('<rect x="3" y="3" width="8" height="8" rx="1"/><rect x="13" y="13" width="8" height="8" rx="1"/>'), false);
    h += '<button class="b sm" data-dg="lay" title="Расставить выделенное автоматически">Выстроить ▾</button>';
  } else if (cx.groups && B.sel.size) h += SEP + btn('ungroup', '', 'Разгруппировать', SVG('<rect x="3" y="3" width="8" height="8" rx="1"/><rect x="13" y="13" width="8" height="8" rx="1"/>'), false);
  el.dg.innerHTML = h;
}
/** Изменить объекты с отменой: fn(копия, исходный). */
function applyTo(list, fn) {
  if (!canEdit() || !list.length) return; const before = new Map(), after = new Map();
  for (const o of list) { const n = copy(o), b0 = copy(o); if (fn(n, o) === false || JSON.stringify(n) === JSON.stringify(b0)) continue; before.set(o.id, b0); after.set(o.id, n); }
  if (after.size) change(before, after);
}
function setDef(key, k, v) { const d = { ...(SET[key] || {}) }; if (v === '' || v === undefined) delete d[k]; else d[k] = v; setSet(key, d); }
function dgClick(e) {
  const b = e.target.closest('[data-dg]'); if (!b) return;
  const a = b.dataset.dg, v = b.dataset.v, cx = dgCx(), kz = kzoom();
  const shapes = cx.shapes, links = cx.links;
  switch (a) {
    case 'kind': shapePicker(b, shapes.length ? 'change' : 'new'); return;
    case 'fill': if (shapes.length) applyTo(shapes, n => { if (v) n.f = v; else delete n.f; }); else setSet('bShapeSt', { ...(SET.bShapeSt || {}), f: v }); break;
    case 'sd': if (shapes.length) applyTo(shapes, n => { if (v) n.sd = v; else delete n.sd; }); else setDef('bShapeSt', 'sd', v); break;
    case 'sw': if (shapes.length) applyTo(shapes, n => { n.sw = Math.round((+v / kz) * 10) / 10; }); else setDef('bShapeSt', 'sw', +v); break;
    case 'lk': if (links.length) applyTo(links, n => { n.k = v; }); setDef('bLinkSt', 'k', v === 'straight' ? '' : v); break;
    case 'lsd': if (links.length) applyTo(links, n => { if (v) n.sd = v; else delete n.sd; }); else setDef('bLinkSt', 'sd', v); break;
    case 'lsw': if (links.length) applyTo(links, n => { n.sw = Math.round((+v / kz) * 10) / 10; }); else setDef('bLinkSt', 'sw', +v); break;
    case 'swap': applyTo(links, (n, o) => { n.a = o.b; n.b = o.a; n.ha = o.hb === undefined ? 'arrow' : o.hb; n.hb = o.ha || 'none'; if (o.lt !== undefined) n.lt = 1 - o.lt; }); break;
    case 'ha': case 'hb': headPicker(b, a, links); return;
    case 'lay': layoutPop(b); return;
    default: arrange(a, v);
  }
  B.dgSig = ''; dirty();
}
function headPicker(anchor, which, links) {
  const cur = links[0] ? (which === 'ha' ? links[0].ha || 'none' : links[0].hb === undefined ? 'arrow' : links[0].hb) : (SET.bLinkSt || {})[which] || (which === 'ha' ? 'none' : 'arrow');
  const p = popHtml(anchor, 'bdheads', DG.HEADS.map(([k, t]) => '<button class="prow hdrow' + (k === cur ? ' on' : '') + '" data-h="' + k + '">' + headIcon(k).replace('class="hdic"', 'class="hdic' + (which === 'ha' ? ' flip' : '') + '"') + '<span>' + t + '</span></button>').join(''));
  if (!p) return; p.classList.add('bdheads');
  p.onclick = e => { const b = e.target.closest('[data-h]'); if (!b) return; const k = b.dataset.h; if (links.length) applyTo(links, n => { n[which] = k; }); setDef('bLinkSt', which, k); closePop(); B.dgSig = ''; dirty(); };
}
function shapePicker(anchor, mode = 'new') {
  if (!anchor) return;
  const cur = mode === 'change' ? (dgCx().shapes[0] || {}).k : B.shapeK;
  const html = '<input class="shq" placeholder="Найти фигуру…" aria-label="Найти фигуру">' + DG.SHAPE_GROUPS.filter(g => mode === 'new' || g[0] !== 'cont').map(g => '<div class="plbl shg">' + g[1] + '</div><div class="shgrid">' + g[2].map(([k, n]) => '<button class="shb' + (k === cur ? ' on' : '') + '" data-k="' + k + '" data-n="' + esc(n.toLowerCase()) + '" title="' + esc(n) + '">' + shapeIcon(k, 26) + '</button>').join('') + '</div>').join('') +
    (mode === 'new' ? '<div class="psep"></div><div class="row" style="gap:6px"><button class="b sm" data-x="tpl" style="flex:1">Шаблоны…</button><button class="b sm" data-x="mm" style="flex:1">Из текста…</button></div>' : '');
  const p = popHtml(anchor, 'bdshapes', html, pp => { pp.style.width = '300px'; pp.classList.add('bdshapes'); });
  if (!p) return;
  const q = p.querySelector('.shq'); setTimeout(() => q.focus(), 0);
  q.oninput = () => { const t = q.value.trim().toLowerCase(); p.querySelectorAll('.shb').forEach(b => b.classList.toggle('hide', !!t && !b.dataset.n.includes(t))); p.querySelectorAll('.shgrid').forEach(g => { const any = [...g.children].some(b => !b.classList.contains('hide')); g.classList.toggle('hide', !any); g.previousElementSibling.classList.toggle('hide', !any); }); };
  q.onkeydown = e => { e.stopPropagation(); if (e.key === 'Enter') { const b = p.querySelector('.shb:not(.hide)'); if (b) b.click(); } };
  p.onclick = e => {
    const x = e.target.closest('[data-x]'); if (x) { closePop(); if (x.dataset.x === 'tpl') templatesModal(); else mermaidModal(); return; }
    const b = e.target.closest('[data-k]'); if (!b) return; const k = b.dataset.k;
    if (mode === 'change') applyTo(dgCx().shapes, n => { n.k = k; if (DG.isContainer(n) !== DG.isContainer({ t: 'shape', k })) return false; });
    else { B.shapeK = k; setSet('bShape', k); setTool('shape'); }
    closePop(); B.dgSig = ''; dirty();
  };
}
function layoutPop(anchor) {
  const L = [['right', 'Деревом вправо', '<path d="M4 12h5M9 6v12M9 6h5M9 18h5M14 6h6M14 18h6"/>'], ['down', 'Деревом вниз', '<path d="M12 4v5M6 9h12M6 9v5M18 9v5M6 14v6M18 14v6"/>'], ['circle', 'По кругу', '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="4" r="1.5"/><circle cx="20" cy="12" r="1.5"/><circle cx="12" cy="20" r="1.5"/><circle cx="4" cy="12" r="1.5"/>'], ['grid', 'Сеткой', '<rect x="4" y="4" width="6" height="6"/><rect x="14" y="4" width="6" height="6"/><rect x="4" y="14" width="6" height="6"/><rect x="14" y="14" width="6" height="6"/>']];
  const p = popHtml(anchor, 'bdarr', L.map(([k, t, ic]) => '<button class="prow hdrow" data-l="' + k + '">' + SVG(ic) + '<span>' + t + '</span></button>').join('') + '<div class="small muted" style="max-width:220px">Дерево строится по связям между выделенными фигурами.</div>');
  if (!p) return; p.classList.add('bdarr');
  p.onclick = e => { const b = e.target.closest('[data-l]'); if (!b) return; closePop(); arrange('lay', b.dataset.l); };
}
const ubox = list => { let b = null; for (const o of list) { const x = box(o); b = b ? [Math.min(b[0], x[0]), Math.min(b[1], x[1]), Math.max(b[2], x[2]), Math.max(b[3], x[3])] : x.slice(); } return b; };
/** Выровнять, распределить, размер, группа, авто-раскладка — для выделенного. */
function arrange(a, v) {
  if (!canEdit()) return;
  const sel = [...B.sel].map(id => B.objs.get(id)).filter(o => o && o.t !== 'link');
  if (a === 'group' || a === 'ungroup') {
    if (a === 'group' && sel.length < 2) return; const g = a === 'group' ? 'g' + Date.now().toString(36) : null;
    applyTo(sel, n => { if (g) n.g = g; else delete n.g; }); toast(g ? 'Сгруппировано: ' + sel.length : 'Группа разобрана', 'ok'); return;
  }
  if (sel.length < 2) return;
  const U = ubox(sel), bx = new Map(sel.map(o => [o.id, box(o)])), mv = new Map();
  if (a === 'al') for (const o of sel) { const b = bx.get(o.id); mv.set(o.id, [v === 'l' ? U[0] - b[0] : v === 'c' ? (U[0] + U[2]) / 2 - (b[0] + b[2]) / 2 : v === 'r' ? U[2] - b[2] : 0, v === 't' ? U[1] - b[1] : v === 'm' ? (U[1] + U[3]) / 2 - (b[1] + b[3]) / 2 : v === 'b' ? U[3] - b[3] : 0]); }
  if (a === 'dist') {
    const h = v === 'h', i0 = h ? 0 : 1, i1 = h ? 2 : 3, list = [...sel].sort((p, q) => bx.get(p.id)[i0] - bx.get(q.id)[i0]);
    const total = list.reduce((s2, o) => s2 + bx.get(o.id)[i1] - bx.get(o.id)[i0], 0), gap = (U[i1] - U[i0] - total) / (list.length - 1); let at = U[i0];
    for (const o of list) { const b = bx.get(o.id); mv.set(o.id, h ? [at - b[0], 0] : [0, at - b[1]]); at += b[i1] - b[i0] + gap; }
  }
  if (a === 'size') { const ref = sel.find(o => o.w) || sel[0]; applyTo(sel.filter(o => o.w !== undefined && o.t !== 'text'), n => { if (v === 'w') n.w = ref.w; else if (n.t !== 'note' || ref.h) n.h = ref.h; }); return; }
  if (a === 'lay') {
    const nodes = sel.filter(o => !DG.isContainer(o)).map(o => { const b = bx.get(o.id); return { id: o.id, w: b[2] - b[0], h: b[3] - b[1] }; }), ids = new Set(nodes.map(n => n.id));
    const edges = [...B.objs.values()].filter(l => l.t === 'link' && l.a && l.b && ids.has(l.a.id) && ids.has(l.b.id)).map(l => [l.a.id, l.b.id]);
    if (v !== 'grid' && v !== 'circle' && !edges.length) toast('Между выделенными нет связей — выстроено сеткой', 'warn');
    const pos = v === 'circle' ? DG.circleLayout(nodes, edges) : v === 'grid' || !edges.length ? DG.gridLayout(nodes) : DG.layered(nodes, edges, v === 'right' ? 'LR' : 'TD');
    for (const n of nodes) { const b = bx.get(n.id), q = pos.get(n.id); mv.set(n.id, [U[0] + q[0] - b[0], U[1] + q[1] - b[1]]); }
  }
  applyTo(sel.filter(o => mv.has(o.id)), (n, o) => { const [dx, dy] = mv.get(o.id); if (Math.abs(dx) < 0.01 && Math.abs(dy) < 0.01) return false; shiftTo(n, o, Math.round(dx * 10) / 10, Math.round(dy * 10) / 10); });
}

// ───────────── шаблоны и «Из текста» (Mermaid) ─────────────
function dgOpt() {
  let n = 0;
  return { measure: (t, fs) => { const c = mctx(); c.font = fontOf({}, fs); return c.measureText(t).width; }, newId: () => newId() + (++n).toString(36), layer: B.layer, color: B.color, fill: bgLight() ? '#FFFFFF' : '#1C2140', z0: Date.now() };
}
/** Рамка временных объектов (ещё не на доске). */
function withTmp(objs, fn) {
  B.tmp = new Map(objs.map(o => [o.id, o]));
  try { for (const o of objs) B.cache.delete(o.id); return fn(); }
  finally { for (const o of objs) B.cache.delete(o.id); B.tmp = null; }
}
function drawPreview(cv, objs) {
  const ctx = cv.getContext('2d'), W = cv.width, H = cv.height; ctx.setTransform(1, 0, 0, 1, 0, 0); ctx.fillStyle = bgColor(); ctx.fillRect(0, 0, W, H);
  if (!objs.length) return;
  withTmp(objs, () => {
    const b = ubox(objs), pad = 16, k = Math.min(1.2, (W - pad * 2) / (b[2] - b[0] || 1), (H - pad * 2) / (b[3] - b[1] || 1));
    ctx.setTransform(k, 0, 0, k, pad + ((W - pad * 2) - (b[2] - b[0]) * k) / 2 - b[0] * k, pad + ((H - pad * 2) - (b[3] - b[1]) * k) / 2 - b[1] * k);
    for (const o of objs.slice().sort(order)) drawObj(ctx, o, k, null, true);
  });
}
/** Положить готовые объекты в центр вида (одним шагом отмены) и выделить. */
function insertObjs(objs) {
  if (!canEdit()) { toast(B.enabled ? 'Доску правит только владелец канала' : 'Доска выключена', 'bad'); return; }
  if (layerHiddenWarn() || !objs.length) return;
  const b = withTmp(objs, () => ubox(objs)), [cx, cy] = S2W(B.W / 2, B.H / 2);
  let dx = Math.round(cx - (b[0] + b[2]) / 2), dy = Math.round(cy - (b[1] + b[3]) / 2);
  // в центре уже что-то есть — справа от всего, что на доске
  const busy = query(b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy).some(o => layerShown(layerOf(o)));
  if (busy) { const wb = worldBounds(); if (wb) { dx = Math.round(wb[2] + 120 - b[0]); dy = Math.round(wb[1] - b[1]); } }
  for (const o of objs) { o.L = B.layer; if (o.t === 'link') { for (const k of ['a', 'b']) if (o[k] && !o[k].id) { o[k].x += dx; o[k].y += dy; } } else { o.x += dx; o.y += dy; } }
  change(new Map(objs.map(o => [o.id, null])), new Map(objs.map(o => [o.id, o])));
  setTool('select'); B.sel = new Set(objs.filter(o => o.t !== 'link').map(o => o.id));
  // показать вставленное целиком
  const nb = [b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy], vb = [S2W(0, 0), S2W(B.W, B.H)];
  if (nb[0] < vb[0][0] || nb[1] < vb[0][1] || nb[2] > vb[1][0] || nb[3] > vb[1][1]) { const z = Math.max(0.05, Math.min(B.view.z, B.W / (nb[2] - nb[0] + 160), B.H / (nb[3] - nb[1] + 160))); B.view.z = z; B.view.x = (nb[0] + nb[2]) / 2 - B.W / 2 / z; B.view.y = (nb[1] + nb[3]) / 2 - B.H / 2 / z; B.follow = ''; }
  dirty();
}
function templatesModal() {
  const html = '<h3 class="dgh3">Шаблоны</h3><div class="small muted" style="margin-bottom:12px">Шаблон встанет в центр доски — дальше правьте как обычные фигуры.</div><div class="tplgrid">' +
    DG.TEMPLATES.map(t => '<button class="tplc" data-t="' + t[0] + '"><canvas width="264" height="150"></canvas><b>' + esc(t[1]) + '</b><span class="small muted">' + esc(t[2]) + '</span></button>').join('') + '</div>' +
    '<div class="row" style="margin-top:12px;justify-content:flex-end"><button class="b" data-c="no">Закрыть</button></div>';
  openModal(HtmlModal, { html, mount: m => {
    m.querySelectorAll('.tplc').forEach(b => { try { drawPreview(b.querySelector('canvas'), DG.buildTemplate(b.dataset.t, 0, 0, dgOpt())); } catch {} });
    m.addEventListener('click', e => { if (e.target.closest('[data-c]')) { closeModal(); return; } const b = e.target.closest('[data-t]'); if (!b) return; closeModal(); try { insertObjs(DG.buildTemplate(b.dataset.t, 0, 0, dgOpt())); } catch (err) { toast(err.message, 'bad'); } });
  } });
}
const plural = (n, a, b, c) => { const m = n % 10, h = n % 100; return n + ' ' + (m === 1 && h !== 11 ? a : m >= 2 && m <= 4 && (h < 12 || h > 14) ? b : c); };
const MM_SAMPLE = 'flowchart TD\n  A([Начало]) --> B[Собрать данные]\n  B --> C{Всё верно?}\n  C -->|да| D[Отправить]\n  C -->|нет| B\n  D --> E([Конец])';
function mermaidModal(text) {
  const html = '<h3 class="dgh3">Диаграмма из текста</h3><div class="small muted" style="margin-bottom:8px">Mermaid (flowchart): <code>A[Шаг]</code>, <code>B{Решение}</code>, <code>C([Начало])</code>, <code>D[(База)]</code>, <code>E((Круг))</code>; связи <code>A --&gt; B</code>, <code>A -.-&gt; B</code>, <code>A ==&gt; B</code>, <code>A --&gt;|подпись| B</code>; направление <code>flowchart LR</code>; группы <code>subgraph … end</code>.</div>' +
    '<div class="mmwrap"><textarea class="mmtext mono" spellcheck="false" aria-label="Текст Mermaid"></textarea><canvas class="mmpv" width="520" height="300"></canvas></div><div class="small mmerr"></div>' +
    '<div class="row" style="margin-top:12px;gap:8px;justify-content:flex-end"><button class="b" data-c="no">Отмена</button><button class="b pri" data-c="go">Положить на доску</button></div>';
  openModal(HtmlModal, { html, mount: m => {
    const ta = m.querySelector('.mmtext'), cv = m.querySelector('.mmpv'), er = m.querySelector('.mmerr'); let t = 0, last = null;
    ta.value = text || SET.bMermaid || MM_SAMPLE;
    const upd = () => { try { last = DG.buildMermaid(ta.value, 0, 0, dgOpt()).objs; const ns = last.filter(o => o.t === 'shape' && !DG.isContainer(o)).length, nf = last.filter(o => DG.isContainer(o)).length, nl = last.filter(o => o.t === 'link').length; er.textContent = plural(ns, 'фигура', 'фигуры', 'фигур') + ', ' + plural(nl, 'связь', 'связи', 'связей') + (nf ? ', ' + plural(nf, 'группа', 'группы', 'групп') : ''); er.className = 'small muted mmerr'; drawPreview(cv, last); } catch (e) { last = null; er.textContent = e.message; er.className = 'small mmerr bad'; drawPreview(cv, []); } };
    ta.oninput = () => { clearTimeout(t); t = setTimeout(upd, 220); };
    ta.onkeydown = e => { e.stopPropagation(); if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) m.querySelector('[data-c="go"]').click(); };
    upd(); setTimeout(() => ta.focus(), 50);
    m.addEventListener('click', e => { const b = e.target.closest('[data-c]'); if (!b) return; if (b.dataset.c === 'no') { closeModal(); return; } upd(); if (!last) return; setSet('bMermaid', ta.value.slice(0, 20000)); closeModal(); insertObjs(DG.buildMermaid(ta.value, 0, 0, dgOpt()).objs); });
  } });
}
/** Выделенное (или вся доска) → текст Mermaid. */
function mermaidOf() {
  const pool = B.sel.size ? [...B.sel].map(id => B.objs.get(id)).filter(Boolean) : [...B.objs.values()].filter(o => layerShown(layerOf(o)));
  const shapes = pool.filter(o => o.t === 'shape'), ids = new Set(shapes.map(o => o.id));
  const links = [...B.objs.values()].filter(l => l.t === 'link' && l.a && l.b && ids.has(l.a.id) && ids.has(l.b.id));
  if (!shapes.length) return null;
  // направление — по тому, как стоят фигуры
  const U = ubox(shapes); return DG.toMermaid(shapes.sort((a, b) => a.y - b.y || a.x - b.x), links, U[2] - U[0] > (U[3] - U[1]) * 1.6 ? 'LR' : 'TD');
}

// ───────────── фон ─────────────
function setBg(p) {
  if (!canEdit()) { toast(B.enabled ? 'Фон меняет тот, кто правит доску' : 'Доска выключена', 'bad'); paintBg(); return; }
  B.bg = { c: p.c !== undefined ? p.c : B.bg.c, l: p.l !== undefined ? p.l : B.bg.l };
  send({ t: 'bg', c: B.bg.c, l: B.bg.l }); paintBg(); dirty();
}
function paintBg() {
  if (!el.bgbox) return;
  const cur = B.bg.c || BG_DEFAULT;
  el.bgbox.innerHTML = BG_PRESETS.map(([c, t]) => '<button class="sw' + (cur.toLowerCase() === c.toLowerCase() ? ' on' : '') + '" data-bg="' + c + '" title="' + t + '" style="background:' + c + '"></button>').join('') +
    '<label class="sw bgpick" title="Свой цвет" style="background:' + cur + '"><input type="color" value="' + cur + '"></label>';
  const inp = el.bgbox.querySelector('input'); inp.onchange = () => setBg({ c: inp.value });
  el.bright.value = String(B.bg.l || 100); el.brightv.textContent = (B.bg.l || 100) + '%';
}

// ───────────── слои ─────────────
function toggleLayers() { B.layersOpen = !B.layersOpen; el.layers.classList.toggle('hide', !B.layersOpen); if (B.layersOpen) el.cfg.classList.add('hide'); paintLayers(); paintTools(); }
function paintLayers() {
  if (!el.layers) return;
  if (!B.layersOpen) { paintTools(); return; }
  const n = id => { let k = 0; for (const o of B.objs.values()) if (layerOf(o) === id) k++; return k; };
  const can = canEdit();
  const rows = [...B.layers].map((l, i) => ({ l, i })).reverse().map(({ l, i }) =>
    '<div class="lyrow' + (l.id === B.layer ? ' on' : '') + '" data-ly="pick" data-id="' + esc(l.id) + '">' +
      '<button class="tool" data-ly="eye" data-id="' + esc(l.id) + '" title="' + (B.hidden.has(l.id) ? 'Показать слой (у вас)' : 'Скрыть слой (только у вас)') + '">' + SVG(B.hidden.has(l.id) ? '<path d="M3 3l18 18M10.6 6.1A9.9 9.9 0 0 1 12 6c6 0 9.5 6 9.5 6a17 17 0 0 1-3 3.6M6.6 6.6C3.9 8.3 2.5 12 2.5 12s3.5 6 9.5 6a9.6 9.6 0 0 0 4-.9"/>' : '<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>') + '</button>' +
      '<span class="lyname grow" title="Двойной щелчок — переименовать">' + esc(l.name) + '</span><span class="small muted">' + n(l.id) + '</span>' +
      '<button class="tool" data-ly="up" data-id="' + esc(l.id) + '" title="Выше"' + (i === B.layers.length - 1 || !can ? ' disabled' : '') + '>' + SVG('<path d="M6 15l6-6 6 6"/>') + '</button>' +
      '<button class="tool" data-ly="down" data-id="' + esc(l.id) + '" title="Ниже"' + (i === 0 || !can ? ' disabled' : '') + '>' + SVG('<path d="M6 9l6 6 6-6"/>') + '</button>' +
      '<button class="tool" data-ly="del" data-id="' + esc(l.id) + '" title="Удалить слой вместе с его рисунками"' + (B.layers.length < 2 || !can ? ' disabled' : '') + '>' + SVG('<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>') + '</button></div>').join('');
  el.layers.innerHTML = '<div class="row"><b>Слои</b><span class="small muted">· сверху — ближе</span><div class="grow"></div><button class="tool" data-bd="layers" title="Закрыть">' + SVG('<path d="M6 6l12 12M18 6L6 18"/>') + '</button></div>' +
    '<div class="lylist">' + rows + '</div>' +
    '<div class="row" style="gap:6px"><button class="b sm" data-ly="add"' + (can ? '' : ' disabled') + '>＋ Слой</button><button class="b sm" data-ly="moveSel" title="Перенести выделенное в активный слой"' + (can && B.sel.size ? '' : ' disabled') + '>Выделенное → сюда</button></div>' +
    '<div class="small muted">Рисуете и стираете — только в активном слое. Скрыть слой — у вас, остальные видят всё. Порядок, имена и состав слоёв — общие.</div>';
  el.layers.querySelectorAll('.lyname').forEach(s => s.ondblclick = ev => { ev.stopPropagation(); const id = s.closest('[data-id]').dataset.id; layerAction('rename', id); });
  paintTools();
}
function sendLayers() { send({ t: 'layers', list: B.layers.map(l => ({ id: l.id, name: l.name })) }); }
function layerAction(a, id, ev) {
  if (ev && a !== 'pick') ev.stopPropagation();
  const i = B.layers.findIndex(l => l.id === id);
  switch (a) {
    case 'pick': B.layer = id; B.sel.clear(); break;
    case 'eye': if (B.hidden.has(id)) B.hidden.delete(id); else B.hidden.add(id); dirty(); break;
    case 'up': if (!canEdit() || i < 0 || i >= B.layers.length - 1) return; [B.layers[i], B.layers[i + 1]] = [B.layers[i + 1], B.layers[i]]; sendLayers(); dirty(); break;
    case 'down': if (!canEdit() || i <= 0) return; [B.layers[i], B.layers[i - 1]] = [B.layers[i - 1], B.layers[i]]; sendLayers(); dirty(); break;
    case 'add': {
      if (!canEdit()) return;
      const nid = 'l' + Date.now().toString(36) + Math.random().toString(36).slice(2, 5);
      B.layers.push({ id: nid, name: 'Слой ' + (B.layers.length + 1) }); B.layer = nid; sendLayers(); break;
    }
    case 'rename': {
      if (!canEdit() || i < 0) return;
      const v = prompt('Название слоя', B.layers[i].name); if (!v || !v.trim()) return;
      B.layers[i] = { ...B.layers[i], name: v.trim().slice(0, 40) }; sendLayers(); break;
    }
    case 'del': {
      if (!canEdit() || i < 0 || B.layers.length < 2) return;
      const objs = [...B.objs.values()].filter(o => layerOf(o) === id);
      if (!confirm('Удалить слой «' + B.layers[i].name + '»' + (objs.length ? ' и всё, что на нём (' + objs.length + ')' : '') + '? Это у всех в канале.')) return;
      if (objs.length) change(new Map(objs.map(o => [o.id, copy(o)])), new Map(objs.map(o => [o.id, null])));
      B.layers.splice(i, 1); fixActive(); sendLayers(); dirty(); break;
    }
    case 'moveSel': {
      if (!canEdit() || !B.sel.size) return;
      const before = new Map(), after = new Map();
      for (const sid of B.sel) { const o = B.objs.get(sid); if (!o || layerOf(o) === B.layer) continue; before.set(sid, copy(o)); after.set(sid, { ...copy(o), L: B.layer }); }
      if (after.size) { change(before, after); toast('Перенесено в «' + activeLayer().name + '»: ' + after.size, 'ok'); }
      B.sel.clear(); dirty(); break;
    }
  }
  paintLayers();
}

// ───────────── экспорт PNG ─────────────
function exportPop(anchor) {
  const st = { what: 'all', bg: 'board', k: 2 };
  const seg = (key, opts) => '<div class="seg" data-ex="' + key + '">' + opts.map(([v, t]) => '<button data-v="' + v + '"' + (String(st[key]) === String(v) ? ' class="on"' : '') + '>' + t + '</button>').join('') + '</div>';
  const p = popHtml(anchor, 'bdexport', '<div class="plbl">Что</div>' + seg('what', [['all', 'Вся доска'], ['vis', 'Что видно'], ['sel', 'Выделенное']]) + '<div class="plbl">Фон</div>' + seg('bg', [['board', 'Как на доске'], ['light', 'Светлый'], ['none', 'Прозрачный']]) +
    '<div class="plbl">Чёткость</div>' + seg('k', [[1, '1×'], [2, '2×'], [3, '3×']]) + '<div class="small muted exinfo"></div><button class="b pri" data-x="png">Скачать PNG</button><div class="row" style="gap:6px"><button class="b sm" data-x="copy" style="flex:1">Копировать</button><button class="b sm" data-x="chat" style="flex:1">В чат канала</button></div><button class="b sm" data-mm="1" title="Фигуры и связи (выделенные или все) — текстом Mermaid">Копировать как Mermaid</button><div class="small muted">Скрытые у вас слои в картинку не попадут.</div>');
  if (!p) return;
  const info = () => { const b = exBounds(st.what); p.querySelector('.exinfo').textContent = b ? 'Картинка ' + Math.round(b.w * b.k) + ' × ' + Math.round(b.h * b.k) : 'Нечего выгружать'; };
  p.querySelectorAll('[data-ex]').forEach(s => (s.onclick = e => { const b = e.target.closest('[data-v]'); if (!b) return; st[s.dataset.ex] = isNaN(+b.dataset.v) ? b.dataset.v : +b.dataset.v; s.querySelectorAll('[data-v]').forEach(x => x.classList.toggle('on', x === b)); B.ex = st; info(); }));
  B.ex = st; info();
  p.querySelector('[data-mm]').onclick = async () => { const t = mermaidOf(); if (!t) { toast('Нет фигур — Mermaid описывает фигуры и связи', 'bad'); return; } try { await navigator.clipboard.writeText(t); toast('Mermaid скопирован', 'ok'); closePop(); } catch { closePop(); mermaidModal(t); } };
  p.querySelectorAll('[data-x]').forEach(b => (b.onclick = async () => {
    b.disabled = true;
    try {
      const blob = await exportPng(st); if (!blob) throw new Error('нечего выгружать');
      const name = 'доска-' + (R.title || 'канал') + '-' + new Date().toISOString().slice(0, 16).replace(/[:T]/g, '-') + '.png';
      if (b.dataset.x === 'png') { const a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = name; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 5000); }
      else if (b.dataset.x === 'copy') { await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]); toast('Картинка доски скопирована', 'ok'); }
      else { await api('POST', 'chat/send', { conv: 'ch:' + B.room, text: '', files: [{ name, mime: 'image/png', data: b64(await blob.arrayBuffer()) }] }); toast('Доска отправлена в чат канала', 'ok'); }
      closePop();
    } catch (e) { toast('Экспорт: ' + e.message, 'bad'); b.disabled = false; }
  }));
}
function exBounds(what) {
  let list; let b = null;
  if (what === 'vis') { const [x0, y0] = S2W(0, 0), [x1, y1] = S2W(B.W, B.H); list = query(x0, y0, x1, y1); b = [x0, y0, x1, y1]; }
  else {
    list = what === 'sel' ? [...B.sel].map(id => B.objs.get(id)).filter(Boolean) : [...B.objs.values()].filter(o => layerShown(layerOf(o))).sort(order);
    for (const o of list) { const x = box(o); b = b ? [Math.min(b[0], x[0]), Math.min(b[1], x[1]), Math.max(b[2], x[2]), Math.max(b[3], x[3])] : x.slice(); }
    if (b) b = [b[0] - 40, b[1] - 40, b[2] + 40, b[3] + 40];
  }
  if (!b || !list.length) return null;
  const w = b[2] - b[0], h = b[3] - b[1]; let k = (B.ex || {}).k || 2; k = Math.min(k, 8192 / w, 8192 / h, Math.sqrt(40e6 / (w * h)));
  return { list, b, w, h, k };
}
async function exportPng(st) {
  const e = exBounds(st.what); if (!e) return null;
  const { list, b, w, h } = e, k = Math.min(st.k, 8192 / w, 8192 / h, Math.sqrt(40e6 / (w * h)));
  const ids = [...new Set(list.filter(o => o.t === 'img').map(o => o.src))];
  await Promise.all(ids.map(id => new Promise(res => { const im = image(id) || { ok: false }; if (!im.img || im.ok) return res(); const t = setTimeout(res, 8000); im.img.addEventListener('load', () => { clearTimeout(t); res(); }, { once: true }); im.img.addEventListener('error', () => { clearTimeout(t); res(); }, { once: true }); })));
  const c = document.createElement('canvas'); c.width = Math.ceil(w * k); c.height = Math.ceil(h * k); const ctx = c.getContext('2d');
  if (st.bg !== 'none') { ctx.fillStyle = st.bg === 'light' ? '#F4F5FA' : bgColor(); ctx.fillRect(0, 0, c.width, c.height); }
  ctx.setTransform(k, 0, 0, k, -b[0] * k, -b[1] * k);
  for (const o of list.slice().sort(order)) drawObj(ctx, o, k, null, true);
  return await new Promise(r => c.toBlob(r, 'image/png'));
}

// ───────────── стенды ─────────────
async function standsPop(anchor) {
  let list = []; try { list = (await api('GET', 'board/stands')).stands || []; } catch (e) { toast(e.message, 'bad'); }
  const def = 'Доска «' + (R.title || '') + '» — ' + new Date().toLocaleDateString('ru-RU', { day: 'numeric', month: 'long' });
  const p = popHtml(anchor, 'bdstands', '<div class="small muted">Стенд — сохранённая доска целиком: рисунки, картинки, заметки, слои и фон. Хранится на этом ПК, загрузить можно в любой канал.</div>' +
    '<input class="stname" value="' + esc(def) + '" aria-label="Название стенда"><div class="row" style="gap:6px"><button class="b pri" data-s="save" style="flex:1"' + (B.objs.size ? '' : ' disabled') + '>Сохранить стенд</button><button class="b sm" data-s="file"' + (B.objs.size ? '' : ' disabled') + ' title="Сохранить и скачать файл .mostboard">В файл…</button></div>' +
    '<div class="psep"></div><div class="plbl">Сохранённые</div><div class="stlist">' + (list.map(s => '<div class="prow"><div class="grow" style="min-width:0"><div style="font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">' + esc(s.name || s.id) + '</div><div class="small muted">' + (s.time ? ago(s.time) + ' · ' : '') + esc(titleOf(s.author || '')) + ' · ' + (s.objects || 0) + ' объектов · ' + size(s.size) + '</div></div><button class="b sm" data-load="' + esc(s.id) + '" data-name="' + esc(s.name || '') + '">Загрузить</button><a class="b sm ib" title="Скачать файл" href="' + aurl('board/stand/file?id=' + encodeURIComponent(s.id)) + '">⬇</a><button class="b sm ib" data-rm="' + esc(s.id) + '" title="Удалить стенд">✕</button></div>').join('') || '<div class="small muted">Пока нет</div>') + '</div>' +
    '<button class="b sm" data-s="open">Открыть файл стенда…</button>', p => { p.style.width = '360px'; });
  if (!p) return;
  const save = async () => api('POST', 'board/stand/save', { room: B.room, name: p.querySelector('.stname').value });
  p.querySelector('[data-s="save"]').onclick = async ev => { ev.target.disabled = true; try { const r = await save(); toast('Стенд «' + r.name + '» сохранён', 'ok'); closePop(); } catch (e) { toast(e.message, 'bad'); ev.target.disabled = false; } };
  p.querySelector('[data-s="file"]').onclick = async () => { try { const r = await save(); const a = document.createElement('a'); a.href = aurl('board/stand/file?id=' + encodeURIComponent(r.id)); a.click(); closePop(); } catch (e) { toast(e.message, 'bad'); } };
  p.querySelector('[data-s="open"]').onclick = () => {
    const inp = document.createElement('input'); inp.type = 'file'; inp.accept = '.mostboard,application/json';
    inp.onchange = async () => { const f = inp.files[0]; if (!f) return; try { const r = await api('POST', 'board/stand/import?name=' + encodeURIComponent(f.name), f, true); closePop(); confirmLoad(r.id, r.name); } catch (e) { toast('Стенд: ' + e.message, 'bad'); } };
    inp.click();
  };
  p.querySelectorAll('[data-load]').forEach(b => (b.onclick = () => { closePop(); confirmLoad(b.dataset.load, b.dataset.name); }));
  p.querySelectorAll('[data-rm]').forEach(b => (b.onclick = async () => { if (!confirm('Удалить стенд с этого ПК?')) return; try { await api('POST', 'board/stand/remove', { id: b.dataset.rm }); b.closest('.prow').remove(); } catch (e) { toast(e.message, 'bad'); } }));
}
function confirmLoad(id, name) {
  if (!B.room) return;
  const html = '<h3 style="font-weight:300;font-family:var(--display);font-size:22px;margin:0 0 8px">Загрузить «' + esc(name || 'стенд') + '»?</h3><div class="small muted">Доска канала «' + esc(R.title) + '» сменится у всех участников. Текущую можно сначала сохранить стендом — ничего не потеряется.</div>' +
    '<div class="row" style="margin-top:14px;gap:8px;justify-content:flex-end;flex-wrap:wrap"><button class="b" data-c="no">Отмена</button>' + (B.objs.size ? '<button class="b" data-c="save">Сохранить текущую и загрузить</button>' : '') + '<button class="b pri" data-c="load">Загрузить</button></div>';
  const go = async saveFirst => {
    try {
      if (saveFirst) await api('POST', 'board/stand/save', { room: B.room, name: 'Доска «' + R.title + '» перед загрузкой — ' + new Date().toLocaleString('ru-RU') });
      await api('POST', 'board/stand/load', { room: B.room, id, cs: 'stand-' + SESSION });
      closeModal(); toast('Стенд загружен', 'ok'); B.fitted = false; setTimeout(fit, 600);
    } catch (e) { toast(e.message, 'bad'); }
  };
  openModal(HtmlModal, { html, mount: el2 => el2.addEventListener('click', e => { const b = e.target.closest('[data-c]'); if (!b) return; if (b.dataset.c === 'no') closeModal(); else go(b.dataset.c === 'save'); }) });
}

// ───────────── снаружи ─────────────
const listeners = new Set();
// ───────────── пульт ассистента: PDF, картинка доски, показать место ─────────────
/** Страницы: [1, 3] или «1-5,8»; пусто — все (до maxPages). */
function pageList(spec, n, max) {
  let out = [];
  if (Array.isArray(spec)) out = spec.map(Number);
  else if (typeof spec === 'string' && spec.trim()) for (const part of spec.split(',')) { const [a, b] = part.split('-').map(x => +x.trim()); if (a) for (let i = a; i <= (b || a); i++) out.push(i); }
  else for (let i = 1; i <= n; i++) out.push(i);
  return [...new Set(out.filter(i => i >= 1 && i <= n))].slice(0, Math.max(1, Math.min(200, +max || 40)));
}
addUi('board_pdf', async a => {
  const pdfjs = await pdfLib();
  const resp = await fetch(aurl('board/tmp?id=' + encodeURIComponent(a.tmp)));
  if (!resp.ok) throw new Error('файл PDF не найден');
  const doc = await pdfjs.getDocument({ data: await resp.arrayBuffer() }).promise;
  const pages = pageList(a.pages, doc.numPages, a.maxPages);
  const W = Math.max(50, +a.w || 800), name = String(a.name || 'PDF').slice(0, 60), objs = [];
  let cy = +a.y || 0;
  try {
    for (const i of pages) {
      const pg = await pdfPage(doc, i, 1800);
      const r = await api('POST', 'board/image?room=' + encodeURIComponent(a.room), pg.blob, true);
      const h = Math.round((W * pg.h) / pg.w);
      const o = { id: 'api-' + Math.random().toString(36).slice(2, 10) + '-' + i, t: 'img', L: a.layer || BASE, src: r.id, x: Math.round(+a.x || 0), y: Math.round(cy), w: Math.round(W), h, z: Date.now() + i, pdf: name, page: i };
      objs.push(o); cy += h + 40;
      if (objs.length % 10 === 0) await api('POST', 'board/op', { room: a.room, cs: 'api', ops: objs.slice(-10).map(o => ({ t: 'put', o })) });
    }
    const rest = objs.length % 10; if (rest) await api('POST', 'board/op', { room: a.room, cs: 'api', ops: objs.slice(-rest).map(o => ({ t: 'put', o })) });
  } finally { try { doc.destroy(); } catch {} }
  return { objs, pages: doc.numPages };
});
const needBoard = a => {
  if (!B.room) throw new Error('панель не в канале — сначала ui join {channel}');
  if (a.room && a.room.toLowerCase() !== B.room.toLowerCase() && !B.room.toLowerCase().endsWith(':' + String(a.room).toLowerCase())) throw new Error('панель в другом канале (' + B.room + ') — сначала ui join {channel}');
  if (!B.loaded) throw new Error('доска ещё загружается — повторите через секунду');
};
/** Что выгружать для ассистента: всё, видимое, объекты по id или область [x0, y0, x1, y1]. */
function apiBounds(a) {
  let list, b = null;
  const ids = Array.isArray(a.ids) ? a.ids : String(a.ids || '').split(',').filter(Boolean);
  if (Array.isArray(a.area) && a.area.length === 4) { const [x0, y0, x1, y1] = a.area.map(Number); b = [Math.min(x0, x1), Math.min(y0, y1), Math.max(x0, x1), Math.max(y0, y1)]; list = query(b[0], b[1], b[2], b[3]).filter(o => layerShown(layerOf(o))); }
  else if (a.what === 'vis') { const [x0, y0] = S2W(0, 0), [x1, y1] = S2W(B.W, B.H); b = [x0, y0, x1, y1]; list = query(x0, y0, x1, y1); }
  else {
    list = ids.length ? ids.map(id => B.objs.get(id)).filter(Boolean) : [...B.objs.values()].filter(o => layerShown(layerOf(o)));
    for (const o of list) { const x = box(o); b = b ? [Math.min(b[0], x[0]), Math.min(b[1], x[1]), Math.max(b[2], x[2]), Math.max(b[3], x[3])] : x.slice(); }
    if (b) b = [b[0] - 40, b[1] - 40, b[2] + 40, b[3] + 40];
  }
  if (!b) return null;
  return { list, b, w: b[2] - b[0], h: b[3] - b[1] };
}
addUi('board_mermaid', async a => {
  let n = 0; const rid = Math.random().toString(36).slice(2, 8);
  const opt = { ...dgOpt(), newId: () => 'api-' + rid + '-' + (++n).toString(36), layer: a.layer || BASE, color: a.color || '#E8EAFB', fill: a.fill !== undefined ? a.fill : '#1C2140' };
  const { objs, dir } = DG.buildMermaid(String(a.text || ''), +a.x || 0, +a.y || 0, opt);
  for (let i = 0; i < objs.length; i += 200) await api('POST', 'board/op', { room: a.room, cs: 'api', ops: objs.slice(i, i + 200).map(o => ({ t: 'put', o })) });
  return { objs, dir };
});
addUi('board_png', async a => {
  needBoard(a);
  const e = apiBounds(a); if (!e) throw new Error('на доске нечего показать');
  const { list, b, w, h } = e, k = Math.min(+a.scale || 1, 4096 / w, 4096 / h, Math.sqrt(16e6 / (w * h)));
  const ids = [...new Set(list.filter(o => o.t === 'img').map(o => o.src))];
  await Promise.all(ids.map(id => new Promise(res => { const im = image(id) || { ok: false }; if (!im.img || im.ok) return res(); const t = setTimeout(res, 8000); im.img.addEventListener('load', () => { clearTimeout(t); res(); }, { once: true }); im.img.addEventListener('error', () => { clearTimeout(t); res(); }, { once: true }); })));
  const c = document.createElement('canvas'); c.width = Math.max(1, Math.ceil(w * k)); c.height = Math.max(1, Math.ceil(h * k)); const ctx = c.getContext('2d');
  if (a.bg !== 'none') { ctx.fillStyle = a.bg === 'light' ? '#F4F5FA' : bgColor(); ctx.fillRect(0, 0, c.width, c.height); }
  ctx.setTransform(k, 0, 0, k, -b[0] * k, -b[1] * k);
  for (const o of list.slice().sort(order)) drawObj(ctx, o, k, null, true);
  const blob = await new Promise(r => c.toBlob(r, 'image/png'));
  const r = await api('POST', 'shots/save?kind=board', blob, true);
  return { path: r.path, width: c.width, height: c.height, area: b.map(v => Math.round(v)), scale: +k.toFixed(3), objects: list.length };
});
addUi('board_view', async a => {
  needBoard(a);
  setTab('voice'); VV.view = 'board';
  await new Promise(r => setTimeout(r, 120)); resize();
  const ids = Array.isArray(a.ids) ? a.ids : String(a.ids || a.id || '').split(',').filter(Boolean);
  let b = null;
  if (ids.length) { for (const id of ids) { const o = B.objs.get(id); if (!o) throw new Error('нет объекта ' + id); const x = box(o); b = b ? [Math.min(b[0], x[0]), Math.min(b[1], x[1]), Math.max(b[2], x[2]), Math.max(b[3], x[3])] : x.slice(); } if (a.select !== false) { B.sel = new Set(ids); setTool('select'); } }
  else if (Array.isArray(a.area) && a.area.length === 4) b = a.area.map(Number);
  if (b) { const pad = 60, z = Math.max(0.02, Math.min(+a.maxZoom || 2, Math.min(B.W / (b[2] - b[0] + pad * 2), B.H / (b[3] - b[1] + pad * 2)))); B.view.z = z; B.view.x = (b[0] + b[2]) / 2 - B.W / 2 / z; B.view.y = (b[1] + b[3]) / 2 - B.H / 2 / z; B.follow = ''; dirty(); }
  else if (a.x !== undefined && a.y !== undefined) { const z = +a.zoom || B.view.z; B.view.z = Math.max(0.02, Math.min(8, z)); B.view.x = +a.x - B.W / 2 / B.view.z; B.view.y = +a.y - B.H / 2 / B.view.z; dirty(); }
  else fit();
  if (a.full) full(true);
  const [x0, y0] = S2W(0, 0), [x1, y1] = S2W(B.W, B.H);
  return { view: { x0: Math.round(x0), y0: Math.round(y0), x1: Math.round(x1), y1: Math.round(y1), zoom: +B.view.z.toFixed(3) } };
});

function emit() { for (const f of listeners) try { f(); } catch {} }
export const Board = {
  /** Интерфейсу: доска включена (бейдж «вкл/выкл» на вкладке). */
  onChange(f) { listeners.add(f); return () => listeners.delete(f); },
  get enabled() { return B.enabled; },
  host(node) { mount(node); },
  room(key) {
    if (key === B.room) return;
    if (B.room) { B.outq = B.outq.filter(o => o.t !== 'cur'); send({ t: 'cur', gone: true }); flush(); }
    B.gen++; B.room = key; B.seq = 0; B.loaded = false; B.err = ''; B.fitted = false; B.objs.clear(); B.cache.clear(); B.cells.clear(); B.big.clear(); B.sel.clear(); B.curs.clear(); B.undo = []; B.redo = []; B.follow = ''; B.wb = null; B.enabled = false;
    B.bg = { c: '', l: 100 }; B.layers = [{ id: BASE, name: 'Слой 1' }]; B.layer = BASE; B.hidden = new Set();
    for (const im of B.imgs.values()) im.img.src = ''; B.imgs.clear();
    if (B.full && el.bd) full(false);
    emit();
    if (!key) return;
    ui(); pollLoop(B.gen);
  },
  visible(on) {
    B.vis = !!on && !!B.room;
    if (on) setTimeout(() => { resize(); if (!B.fitted && B.objs.size && B.W > 0) { B.fitted = true; fit(); } }, 0);
    else if (B.room) { send({ t: 'cur', gone: true }); if (B.full) full(false); }
    dirty();
  },
  redraw() { dirty(); },
  get state() { return { room: B.room, seq: B.seq, objects: B.objs.size, enabled: B.enabled, drawn: B.drawn, view: { ...B.view }, images: B.imgs.size, layers: B.layers.map(l => l.name), layer: B.layer, bg: { ...B.bg } }; },
  _B: B, _dg: { plusOf: o => plusOf(o), portsOf: o => portsOf(o), box: o => box(o) },
};
(window as any).Board = Board;
