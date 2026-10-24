// @ts-nocheck — геометрия диаграмм доски: фигуры, связи, раскладка, Mermaid. Без состояния: доска передаёт, где брать объекты.
// Фигура: {t:'shape', k, x, y, w, h, f (заливка), c (контур), sw, sd ('' | dash | dot), a (прозрачность), text, va, tc, ff/fb/fi/fu/fa/fs, g}
// Связь: {t:'link', a: {id, s?} | {x, y}, b: …, k: straight | elbow | curve, ha, hb (концы), c, sw, sd, text, lt (0…1, где подпись), fs…}
// s — сторона t/r/b/l; без неё конец сам выбирает ближайшую сторону к другому концу.

// ───────────── фигуры ─────────────
export const SHAPE_GROUPS = [
  ['basic', 'Базовые', [['rect', 'Прямоугольник'], ['round', 'Скруглённый'], ['ellipse', 'Эллипс'], ['diamond', 'Ромб'], ['triangle', 'Треугольник'], ['hexagon', 'Шестиугольник'], ['parallelogram', 'Параллелограмм'], ['trapezoid', 'Трапеция']]],
  ['flow', 'Блок-схема', [['terminal', 'Начало / конец'], ['document', 'Документ'], ['database', 'База данных'], ['predefined', 'Подпроцесс'], ['manual', 'Ручной ввод'], ['delay', 'Задержка'], ['offpage', 'Переход на другую страницу']]],
  ['more', 'Ещё', [['cloud', 'Облако'], ['callout', 'Выноска'], ['star', 'Звезда'], ['arrowR', 'Стрелка-блок'], ['cross', 'Плюс']]],
  ['cont', 'Контейнеры', [['frame', 'Рамка с заголовком'], ['lane', 'Дорожка']]],
];
export const SHAPE_NAME = Object.fromEntries(SHAPE_GROUPS.flatMap(g => g[2]));
export const isContainer = o => o && o.t === 'shape' && (o.k === 'frame' || o.k === 'lane');
export const HEAD = 32, LANE_HEAD = 40;
/** Размер новой фигуры по умолчанию (в единицах доски при масштабе 100%). */
export function shapeSize(k) {
  return { ellipse: [140, 90], diamond: [150, 100], triangle: [130, 110], hexagon: [160, 90], terminal: [160, 64], database: [120, 110], cloud: [180, 110], star: [120, 115], cross: [100, 100], arrowR: [170, 90], frame: [520, 340], lane: [760, 200], callout: [180, 100], document: [160, 96] }[k] || [160, 80];
}

/** Контур фигуры для рисования. */
export function shapePath(o) {
  const { x, y } = o, w = o.w, h = o.h, p = new Path2D(), k = o.k || 'rect';
  const poly = pts => { p.moveTo(x + pts[0][0], y + pts[0][1]); for (let i = 1; i < pts.length; i++) p.lineTo(x + pts[i][0], y + pts[i][1]); p.closePath(); };
  switch (k) {
    case 'round': p.roundRect(x, y, w, h, Math.min(16, h / 3, w / 3)); break;
    case 'terminal': p.roundRect(x, y, w, h, Math.min(h / 2, w / 2)); break;
    case 'ellipse': p.ellipse(x + w / 2, y + h / 2, w / 2, h / 2, 0, 0, Math.PI * 2); break;
    case 'cloud': {
      const bumps = [[0.25, 0.62, 0.25, 0.3], [0.38, 0.36, 0.22, 0.3], [0.62, 0.32, 0.25, 0.3], [0.78, 0.55, 0.22, 0.28], [0.55, 0.7, 0.3, 0.28]];
      for (const [cx, cy, rx, ry] of bumps) { p.moveTo(x + (cx + rx) * w, y + cy * h); p.ellipse(x + cx * w, y + cy * h, rx * w, ry * h, 0, 0, Math.PI * 2); }
      break;
    }
    case 'document': { const wv = Math.min(h * 0.14, 18); p.moveTo(x, y); p.lineTo(x + w, y); p.lineTo(x + w, y + h - wv); p.bezierCurveTo(x + w * 0.75, y + h - wv * 2.6, x + w * 0.5, y + h + wv * 0.6, x + w * 0.25, y + h - wv * 0.4); p.quadraticCurveTo(x + w * 0.1, y + h - wv * 1.4, x, y + h - wv); p.closePath(); break; }
    case 'database': { const ry = Math.min(h * 0.14, 18); p.moveTo(x, y + ry); p.ellipse(x + w / 2, y + ry, w / 2, ry, 0, Math.PI, 0, false); p.lineTo(x + w, y + h - ry); p.ellipse(x + w / 2, y + h - ry, w / 2, ry, 0, 0, Math.PI, false); p.closePath(); p.moveTo(x + w, y + ry); p.ellipse(x + w / 2, y + ry, w / 2, ry, 0, 0, Math.PI, false); break; }
    case 'delay': { const r = Math.min(h / 2, w / 2); p.moveTo(x, y); p.lineTo(x + w - r, y); p.arc(x + w - r, y + h / 2, h / 2, -Math.PI / 2, Math.PI / 2); p.lineTo(x, y + h); p.closePath(); break; }
    case 'predefined': { const d = Math.min(14, w * 0.1); p.rect(x, y, w, h); p.moveTo(x + d, y); p.lineTo(x + d, y + h); p.moveTo(x + w - d, y); p.lineTo(x + w - d, y + h); break; }
    case 'callout': { const r = Math.min(12, h / 4), bh = h * 0.78; p.moveTo(x + r, y); p.arcTo(x + w, y, x + w, y + bh, r); p.arcTo(x + w, y + bh, x, y + bh, r); p.lineTo(x + w * 0.38, y + bh); p.lineTo(x + w * 0.18, y + h); p.lineTo(x + w * 0.22, y + bh); p.arcTo(x, y + bh, x, y, r); p.arcTo(x, y, x + w, y, r); p.closePath(); break; }
    case 'frame': p.roundRect(x, y, w, h, 10); p.moveTo(x, y + HEAD); p.lineTo(x + w, y + HEAD); break;
    case 'lane': p.rect(x, y, w, h); p.moveTo(x + LANE_HEAD, y); p.lineTo(x + LANE_HEAD, y + h); break;
    case 'rect': if (o.r) p.roundRect(x, y, w, h, Math.min(o.r, w / 2, h / 2)); else p.rect(x, y, w, h); break;
    default: poly(outlinePts(k, w, h));
  }
  return p;
}
/** Многоугольник контура (для пересечений и точек привязки); кривые — ломаной. */
function outlinePts(k, w, h) {
  const E = (n = 40) => Array.from({ length: n }, (_, i) => [w / 2 + (w / 2) * Math.cos((i / n) * Math.PI * 2), h / 2 + (h / 2) * Math.sin((i / n) * Math.PI * 2)]);
  switch (k) {
    case 'diamond': return [[w / 2, 0], [w, h / 2], [w / 2, h], [0, h / 2]];
    case 'triangle': return [[w / 2, 0], [w, h], [0, h]];
    case 'hexagon': { const d = Math.min(w * 0.22, h * 0.5); return [[d, 0], [w - d, 0], [w, h / 2], [w - d, h], [d, h], [0, h / 2]]; }
    case 'parallelogram': { const d = Math.min(w * 0.2, h * 0.6); return [[d, 0], [w, 0], [w - d, h], [0, h]]; }
    case 'trapezoid': { const d = Math.min(w * 0.2, h * 0.6); return [[d, 0], [w - d, 0], [w, h], [0, h]]; }
    case 'manual': return [[0, h * 0.25], [w, 0], [w, h], [0, h]];
    case 'offpage': return [[0, 0], [w, 0], [w, h * 0.7], [w / 2, h], [0, h * 0.7]];
    case 'arrowR': { const hw = Math.min(w * 0.4, h); return [[0, h * 0.25], [w - hw, h * 0.25], [w - hw, 0], [w, h / 2], [w - hw, h], [w - hw, h * 0.75], [0, h * 0.75]]; }
    case 'cross': { const a = w / 3, b = h / 3; return [[a, 0], [2 * a, 0], [2 * a, b], [w, b], [w, 2 * b], [2 * a, 2 * b], [2 * a, h], [a, h], [a, 2 * b], [0, 2 * b], [0, b], [a, b]]; }
    case 'star': return Array.from({ length: 10 }, (_, i) => { const r = i % 2 ? 0.4 : 1, a = -Math.PI / 2 + (i * Math.PI) / 5; return [w / 2 + (w / 2) * r * Math.cos(a), h * 0.53 + (h * 0.53) * r * Math.sin(a)]; });
    case 'ellipse': case 'cloud': return E();
    case 'terminal': { const r = Math.min(h / 2, w / 2), out = []; for (let i = 0; i <= 12; i++) { const a = -Math.PI / 2 + (i / 12) * Math.PI; out.push([w - r + r * Math.cos(a), h / 2 + r * Math.sin(a)]); } for (let i = 0; i <= 12; i++) { const a = Math.PI / 2 + (i / 12) * Math.PI; out.push([r + r * Math.cos(a), h / 2 + r * Math.sin(a)]); } return out; }
    default: return [[0, 0], [w, 0], [w, h], [0, h]];
  }
}
/** Где выписывать текст внутри фигуры: [x, y, w, h]. */
export function textBox(o) {
  const { x, y, w, h } = o, k = o.k, P = 10;
  switch (k) {
    case 'frame': return [x + 12, y, w - 24, HEAD];
    case 'lane': return [x, y, LANE_HEAD, h];
    case 'diamond': return [x + w * 0.2, y + h * 0.2, w * 0.6, h * 0.6];
    case 'ellipse': case 'cloud': return [x + w * 0.15, y + h * 0.15, w * 0.7, h * 0.7];
    case 'triangle': return [x + w * 0.25, y + h * 0.45, w * 0.5, h * 0.5];
    case 'star': return [x + w * 0.3, y + h * 0.35, w * 0.4, h * 0.38];
    case 'hexagon': case 'parallelogram': case 'trapezoid': { const d = Math.min(w * 0.2, h * 0.6); return [x + d, y + P / 2, w - d * 2, h - P]; }
    case 'database': { const ry = Math.min(h * 0.14, 18); return [x + P, y + ry * 2, w - P * 2, h - ry * 3]; }
    case 'document': return [x + P, y + P / 2, w - P * 2, h - Math.min(h * 0.14, 18) - P];
    case 'callout': return [x + P, y + P / 2, w - P * 2, h * 0.78 - P];
    case 'predefined': { const d = Math.min(14, w * 0.1); return [x + d + 6, y + P / 2, w - d * 2 - 12, h - P]; }
    case 'arrowR': return [x + P, y + h * 0.25, w - Math.min(w * 0.4, h) * 0.6 - P, h * 0.5];
    case 'cross': return [x + w / 3, y + h / 3, w / 3, h / 3];
    case 'delay': return [x + P, y + P / 2, w - h / 2 - P, h - P];
    default: return [x + P, y + P / 2, w - P * 2, h - P];
  }
}

// ───────────── привязка ─────────────
export const SIDES = { t: [0, -1], r: [1, 0], b: [0, 1], l: [-1, 0] };
/** Объекты, к которым можно привязать связь. */
export const attachable = o => o && !['link', 'path', 'arrow'].includes(o.t);
/** Пересечение луча из центра рамки с контуром. */
function rayHit(o, bx, dx, dy) {
  const [x0, y0, x1, y1] = bx, w = x1 - x0, h = y1 - y0, cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
  const pts = o.t === 'shape' && !isContainer(o) ? outlinePts(o.k || 'rect', w, h) : [[0, 0], [w, 0], [w, h], [0, h]];
  let best = null;
  for (let i = 0; i < pts.length; i++) {
    const [ax, ay] = pts[i], [bx2, by2] = pts[(i + 1) % pts.length];
    const ex = bx2 - ax, ey = by2 - ay, den = dx * ey - dy * ex; if (Math.abs(den) < 1e-9) continue;
    const qx = x0 + ax - cx, qy = y0 + ay - cy, t = (qx * ey - qy * ex) / den, u = (qx * dy - qy * dx) / den;
    if (t > 0 && u >= -1e-6 && u <= 1 + 1e-6 && (best === null || t < best)) best = t;
  }
  if (best === null) best = Math.min(Math.abs(w / 2 / (dx || 1e-9)), Math.abs(h / 2 / (dy || 1e-9)));
  return [cx + dx * best, cy + dy * best];
}
/** Точка привязки на стороне s. */
export function portPt(o, bx, s) { const [dx, dy] = SIDES[s]; return rayHit(o, bx, dx, dy); }
/** Ближайшая сторона к точке. */
export function nearSide(bx, px, py) {
  const cx = (bx[0] + bx[2]) / 2, cy = (bx[1] + bx[3]) / 2, w = Math.max(1, bx[2] - bx[0]), h = Math.max(1, bx[3] - bx[1]);
  const nx = (px - cx) / w, ny = (py - cy) / h;
  return Math.abs(nx) > Math.abs(ny) ? (nx > 0 ? 'r' : 'l') : ny > 0 ? 'b' : 't';
}
const center = bx => [(bx[0] + bx[2]) / 2, (bx[1] + bx[3]) / 2];

/**
 * Геометрия связи. get(id) — объект, boxOf(o) — его рамка.
 * Возвращает {poly: [[x,y],…] (кривая — ломаной), path: {k, pts | bez}, a: [x,y,угол наружу], b: […], ok}.
 */
export function linkGeom(l, get, boxOf) {
  const A = l.a || {}, Bn = l.b || {};
  const oa = A.id ? get(A.id) : null, ob = Bn.id ? get(Bn.id) : null;
  const ba = oa && attachable(oa) ? boxOf(oa) : null, bb = ob && attachable(ob) ? boxOf(ob) : null;
  const ref = (n, bx) => (bx ? (n.s ? portPt(n === A ? oa : ob, bx, n.s) : center(bx)) : [+n.x || 0, +n.y || 0]);
  const ra = ref(A, ba), rb = ref(Bn, bb);
  const k = l.k || 'straight';
  // концы и направления наружу
  const end = (n, bx, o, me, other) => {
    if (!bx) { const dx = other[0] - me[0], dy = other[1] - me[1]; if (k === 'straight') { const d = Math.hypot(dx, dy) || 1; return { p: me, d: [dx / d, dy / d] }; } return { p: me, d: Math.abs(dx) > Math.abs(dy) ? [Math.sign(dx) || 1, 0] : [0, Math.sign(dy) || 1] }; }
    if (n.s) return { p: portPt(o, bx, n.s), d: SIDES[n.s] };
    if (k === 'straight') { const c = center(bx), dx = other[0] - c[0], dy = other[1] - c[1], d = Math.hypot(dx, dy) || 1; return { p: rayHit(o, bx, dx / d, dy / d), d: [dx / d, dy / d] }; }
    const s = nearSide(bx, other[0], other[1]); return { p: portPt(o, bx, s), d: SIDES[s] };
  };
  const ea = end(A, ba, oa, ra, rb), eb = end(Bn, bb, ob, rb, ra);
  let poly, path;
  if (k === 'curve') {
    const dist = Math.hypot(eb.p[0] - ea.p[0], eb.p[1] - ea.p[1]), m = Math.max(30, dist * 0.42);
    const c1 = [ea.p[0] + ea.d[0] * m, ea.p[1] + ea.d[1] * m], c2 = [eb.p[0] + eb.d[0] * m, eb.p[1] + eb.d[1] * m];
    path = { k, bez: [ea.p, c1, c2, eb.p] };
    poly = []; for (let i = 0; i <= 32; i++) { const t = i / 32, u = 1 - t; poly.push([u * u * u * ea.p[0] + 3 * u * u * t * c1[0] + 3 * u * t * t * c2[0] + t * t * t * eb.p[0], u * u * u * ea.p[1] + 3 * u * u * t * c1[1] + 3 * u * t * t * c2[1] + t * t * t * eb.p[1]]); }
  } else if (k === 'elbow') {
    poly = elbow(ea, eb); path = { k, pts: poly };
  } else { poly = [ea.p, eb.p]; path = { k, pts: poly }; }
  const ang = (p, q) => Math.atan2(p[1] - q[1], p[0] - q[0]);
  const aIn = k === 'curve' ? ang(ea.p, path.bez[1]) : ang(poly[0], poly[1]);
  const bIn = k === 'curve' ? ang(eb.p, path.bez[2]) : ang(poly[poly.length - 1], poly[poly.length - 2]);
  return { poly, path, a: [ea.p[0], ea.p[1], aIn], b: [eb.p[0], eb.p[1], bIn], attA: !!ba, attB: !!bb };
}
function elbow(ea, eb) {
  const S = 22, s = [ea.p[0] + ea.d[0] * S, ea.p[1] + ea.d[1] * S], e = [eb.p[0] + eb.d[0] * S, eb.p[1] + eb.d[1] * S];
  const ha = ea.d[0] !== 0, hb = eb.d[0] !== 0; let mid;
  if (ha && hb) { const mx = (s[0] + e[0]) / 2; mid = [[mx, s[1]], [mx, e[1]]]; }
  else if (!ha && !hb) { const my = (s[1] + e[1]) / 2; mid = [[s[0], my], [e[0], my]]; }
  else if (ha) mid = [[e[0], s[1]]];
  else mid = [[s[0], e[1]]];
  const out = [ea.p, s, ...mid, e, eb.p], res = [];
  for (const p of out) { const q = res[res.length - 1]; if (q && Math.abs(q[0] - p[0]) < 0.01 && Math.abs(q[1] - p[1]) < 0.01) continue; res.push(p); }
  // убрать точки посреди прямых
  for (let i = res.length - 2; i > 0; i--) { const a = res[i - 1], b = res[i], c = res[i + 1]; if ((Math.abs(a[0] - b[0]) < 0.01 && Math.abs(b[0] - c[0]) < 0.01) || (Math.abs(a[1] - b[1]) < 0.01 && Math.abs(b[1] - c[1]) < 0.01)) res.splice(i, 1); }
  return res;
}
/** Точка на ломаной по доле длины t (0…1). */
export function along(poly, t) {
  let L = 0; for (let i = 1; i < poly.length; i++) L += Math.hypot(poly[i][0] - poly[i - 1][0], poly[i][1] - poly[i - 1][1]);
  let need = L * Math.max(0, Math.min(1, t));
  for (let i = 1; i < poly.length; i++) { const d = Math.hypot(poly[i][0] - poly[i - 1][0], poly[i][1] - poly[i - 1][1]); if (need <= d || i === poly.length - 1) { const k = d ? Math.min(1, need / d) : 0; return [poly[i - 1][0] + (poly[i][0] - poly[i - 1][0]) * k, poly[i - 1][1] + (poly[i][1] - poly[i - 1][1]) * k]; } need -= d; }
  return poly[0];
}
/** Доля длины ближайшей к точке. */
export function nearT(poly, px, py) {
  let L = 0, best = 1e18, bt = 0.5; const segs = [];
  for (let i = 1; i < poly.length; i++) { const d = Math.hypot(poly[i][0] - poly[i - 1][0], poly[i][1] - poly[i - 1][1]); segs.push([L, d]); L += d; }
  for (let i = 1; i < poly.length; i++) {
    const [ax, ay] = poly[i - 1], [bx, by] = poly[i], dx = bx - ax, dy = by - ay, l2 = dx * dx + dy * dy; let t = l2 ? ((px - ax) * dx + (py - ay) * dy) / l2 : 0; t = Math.max(0, Math.min(1, t));
    const d = Math.hypot(px - ax - t * dx, py - ay - t * dy); if (d < best) { best = d; bt = L ? (segs[i - 1][0] + t * segs[i - 1][1]) / L : 0.5; }
  }
  return bt;
}

// ───────────── концы связи ─────────────
export const HEADS = [['none', 'Нет'], ['arrow', 'Стрелка'], ['tri', 'Треугольник'], ['diamond', 'Ромб'], ['circle', 'Круг'], ['one', '«Один» (ER)'], ['many', '«Много» (ER)']];
/** Насколько укоротить линию у конца (чтобы не торчала из-под конца). */
export function headInset(kind, sw) { const s = Math.max(8, sw * 3.2); return kind === 'tri' ? s * 1.1 : kind === 'diamond' ? s * 1.9 : kind === 'circle' ? s * 0.9 : 0; }
/** Конец в точке (x, y); ang — направление наружу от линии (куда «смотрит» конец). */
export function drawHead(ctx, kind, x, y, ang, sw, col, bg) {
  if (!kind || kind === 'none') return;
  const s = Math.max(8, sw * 3.2), ca = Math.cos(ang), sa = Math.sin(ang);
  const P = (f, q) => [x - ca * f + -sa * q, y - sa * f + ca * q]; // f — назад вдоль линии, q — поперёк
  ctx.save(); ctx.lineWidth = sw; ctx.strokeStyle = col; ctx.fillStyle = col; ctx.lineJoin = 'round'; ctx.lineCap = 'round'; ctx.setLineDash([]);
  ctx.beginPath();
  switch (kind) {
    case 'arrow': { const a = P(s * 1.1, -s * 0.55), b = P(s * 1.1, s * 0.55); ctx.moveTo(a[0], a[1]); ctx.lineTo(x, y); ctx.lineTo(b[0], b[1]); ctx.stroke(); break; }
    case 'tri': { const a = P(s * 1.1, -s * 0.6), b = P(s * 1.1, s * 0.6); ctx.moveTo(x, y); ctx.lineTo(a[0], a[1]); ctx.lineTo(b[0], b[1]); ctx.closePath(); ctx.fillStyle = bg; ctx.fill(); ctx.stroke(); break; }
    case 'diamond': { const a = P(s * 0.95, -s * 0.5), b = P(s * 1.9, 0), c = P(s * 0.95, s * 0.5); ctx.moveTo(x, y); ctx.lineTo(a[0], a[1]); ctx.lineTo(b[0], b[1]); ctx.lineTo(c[0], c[1]); ctx.closePath(); ctx.fill(); ctx.stroke(); break; }
    case 'circle': { const c = P(s * 0.45, 0); ctx.arc(c[0], c[1], s * 0.45, 0, Math.PI * 2); ctx.fill(); break; }
    case 'one': { for (const f of [s * 0.6, s * 1.05]) { const a = P(f, -s * 0.6), b = P(f, s * 0.6); ctx.moveTo(a[0], a[1]); ctx.lineTo(b[0], b[1]); } ctx.stroke(); break; }
    case 'many': { const a = P(s * 1.2, 0), l = P(0, -s * 0.65), r = P(0, s * 0.65), bar1 = P(s * 1.45, -s * 0.6), bar2 = P(s * 1.45, s * 0.6); ctx.moveTo(l[0], l[1]); ctx.lineTo(a[0], a[1]); ctx.lineTo(r[0], r[1]); ctx.moveTo(a[0], a[1]); ctx.lineTo(x, y); ctx.moveTo(bar1[0], bar1[1]); ctx.lineTo(bar2[0], bar2[1]); ctx.stroke(); break; }
  }
  ctx.restore();
}
export const dashOf = (sd, sw) => (sd === 'dash' ? [sw * 3.2, sw * 2.4] : sd === 'dot' ? [0.1, sw * 2.4] : []);

// ───────────── раскладка ─────────────
/**
 * Слоями (дерево / блок-схема): nodes [{id, w, h}], edges [[a, b]], dir TD | LR | BT | RL.
 * Возвращает Map id → [x, y] (левый верхний угол), начало — (0, 0).
 */
export function layered(nodes, edges, dir = 'TD', gx = 60, gy = 80) {
  const ids = nodes.map(n => n.id), N = new Map(nodes.map(n => [n.id, n]));
  const out = new Map(ids.map(i => [i, []])), inn = new Map(ids.map(i => [i, []]));
  // циклы: обход в глубину, обратные рёбра не считаем
  const st = new Map(), keep = [];
  const dfs = u => { st.set(u, 1); for (const [a, b] of edges) if (a === u && N.has(b)) { if (st.get(b) === 1) continue; keep.push([a, b]); if (!st.has(b)) dfs(b); } st.set(u, 2); };
  const roots0 = ids.filter(i => !edges.some(([a, b]) => b === i && a !== i && N.has(a)));
  for (const r of [...roots0, ...ids]) if (!st.has(r)) dfs(r);
  const E = []; const seen = new Set(); for (const e of keep) { const k = e[0] + '\u0001' + e[1]; if (!seen.has(k) && e[0] !== e[1]) { seen.add(k); E.push(e); out.get(e[0]).push(e[1]); inn.get(e[1]).push(e[0]); } }
  // ранг — самый длинный путь от корня
  const rank = new Map(); const order = [];
  { const deg = new Map(ids.map(i => [i, inn.get(i).length])), q = ids.filter(i => !deg.get(i)); while (q.length) { const u = q.shift(); order.push(u); for (const v of out.get(u)) { deg.set(v, deg.get(v) - 1); if (!deg.get(v)) q.push(v); } } }
  for (const u of order) rank.set(u, Math.max(0, ...inn.get(u).map(p => rank.get(p) + 1)));
  const R = Math.max(0, ...rank.values()), rows = Array.from({ length: R + 1 }, () => []);
  for (const u of ids) rows[rank.get(u)].push(u);
  // порядок в ряду — по центрам соседей (несколько проходов)
  const pos = () => { const m = new Map(); rows.forEach(r => r.forEach((u, i) => m.set(u, i))); return m; };
  for (let it = 0; it < 6; it++) {
    const p = pos(), down = it % 2 === 0;
    for (let ri = down ? 1 : R - 1; down ? ri <= R : ri >= 0; ri += down ? 1 : -1) {
      const nb = u => (down ? inn.get(u) : out.get(u)); const bc = u => { const l = nb(u); return l.length ? l.reduce((s, v) => s + p.get(v), 0) / l.length : p.get(u); };
      rows[ri].sort((a, b) => bc(a) - bc(b)); rows[ri].forEach((u, i) => p.set(u, i));
    }
  }
  const horiz = dir === 'LR' || dir === 'RL';
  const W = u => (horiz ? N.get(u).h : N.get(u).w), H = u => (horiz ? N.get(u).w : N.get(u).h);
  const gapAcross = horiz ? gy * 0.5 + 10 : gx, gapAlong = horiz ? gx + 30 : gy;
  // поперёк: ставим подряд, затем подтягиваем к центру родителей без наложений
  const cx = new Map();
  rows.forEach(r => { let x = 0; for (const u of r) { cx.set(u, x + W(u) / 2); x += W(u) + gapAcross; } });
  for (let it = 0; it < 4; it++) for (let ri = 1; ri <= R; ri++) {
    const r = rows[ri];
    const want = r.map(u => { const l = inn.get(u); return l.length ? l.reduce((s, v) => s + cx.get(v), 0) / l.length : cx.get(u); });
    for (let i = 0; i < r.length; i++) cx.set(r[i], want[i]);
    for (let i = 1; i < r.length; i++) { const a = r[i - 1], b = r[i], min = cx.get(a) + W(a) / 2 + gapAcross + W(b) / 2; if (cx.get(b) < min) cx.set(b, min); }
    // родители тоже к детям (для ряда 0 — по центру детей)
    if (ri === 1) { const r0 = rows[0]; for (const u of r0) { const l = out.get(u); if (l.length) cx.set(u, l.reduce((s, v) => s + cx.get(v), 0) / l.length); } for (let i = 1; i < r0.length; i++) { const a = r0[i - 1], b = r0[i], min = cx.get(a) + W(a) / 2 + gapAcross + W(b) / 2; if (cx.get(b) < min) cx.set(b, min); } }
  }
  let minX = Infinity; for (const u of ids) minX = Math.min(minX, cx.get(u) - W(u) / 2);
  const res = new Map(); let y = 0;
  const rowsSeq = dir === 'BT' || dir === 'RL' ? [...rows].reverse() : rows;
  for (const r of rowsSeq) {
    const rh = Math.max(0, ...r.map(H));
    for (const u of r) { const a = cx.get(u) - W(u) / 2 - minX, b = y + (rh - H(u)) / 2; res.set(u, horiz ? [b, a] : [a, b]); }
    y += rh + gapAlong;
  }
  return res;
}
/** По кругу: первый — в центр, если у него больше всего связей, иначе все по кругу. */
export function circleLayout(nodes, edges) {
  const deg = new Map(nodes.map(n => [n.id, 0])); for (const [a, b] of edges) { if (deg.has(a)) deg.set(a, deg.get(a) + 1); if (deg.has(b)) deg.set(b, deg.get(b) + 1); }
  const sorted = [...nodes].sort((a, b) => deg.get(b.id) - deg.get(a.id)); const hub = sorted.length > 3 && deg.get(sorted[0].id) >= sorted.length - 1 ? sorted[0] : null;
  const ring = hub ? nodes.filter(n => n !== hub) : nodes, maxD = Math.max(...nodes.map(n => Math.hypot(n.w, n.h)));
  const R = Math.max(maxD * 1.1, (ring.length * maxD * 0.75) / (2 * Math.PI)), res = new Map();
  ring.forEach((n, i) => { const a = -Math.PI / 2 + (i / ring.length) * Math.PI * 2; res.set(n.id, [R + R * Math.cos(a) - n.w / 2, R + R * Math.sin(a) - n.h / 2]); });
  if (hub) res.set(hub.id, [R - hub.w / 2, R - hub.h / 2]);
  return res;
}
/** Сеткой: по рядам, примерно квадрат. */
export function gridLayout(nodes, gap = 50) {
  const n = nodes.length, cols = Math.max(1, Math.ceil(Math.sqrt(n))), cw = Math.max(...nodes.map(o => o.w)), ch = Math.max(...nodes.map(o => o.h)), res = new Map();
  nodes.forEach((o, i) => res.set(o.id, [(i % cols) * (cw + gap) + (cw - o.w) / 2, Math.floor(i / cols) * (ch + gap) + (ch - o.h) / 2]));
  return res;
}

// ───────────── Mermaid ─────────────
const BR = [['(((', ')))', 'ellipse'], ['([', '])', 'terminal'], ['[[', ']]', 'predefined'], ['[(', ')]', 'database'], ['((', '))', 'ellipse'], ['{{', '}}', 'hexagon'], ['[/', '\\]', 'trapezoid'], ['[\\', '/]', 'trapezoid'], ['[/', '/]', 'parallelogram'], ['[\\', '\\]', 'parallelogram'], ['[', ']', 'rect'], ['(', ')', 'round'], ['{', '}', 'diamond'], ['>', ']', 'callout']];
const label = s => String(s).trim().replace(/^"(.*)"$/s, '$1').replace(/<br\s*\/?>/gi, '\n').replace(/\*\*(.*?)\*\*/g, '$1').replace(/&quot;/g, '"').replace(/&amp;/g, '&').replace(/#quot;/g, '"');
/**
 * flowchart / graph → {dir, nodes: [{id, text, k}], edges: [{a, b, text, dash, thick, ha, hb}], groups: [{id, title, nodes}]}.
 * Понимает узлы всех основных форм, цепочки «A --> B --> C», «A & B --> C», подписи «-- текст -->» и «-->|текст|», subgraph.
 */
export function parseMermaid(src) {
  const lines = []; { let cur = '', q = false; for (const ch of String(src || '').replace(/\r/g, '')) { if (ch === '"') q = !q; if (!q && (ch === '\n' || ch === ';')) { lines.push(cur); cur = ''; } else cur += ch; } lines.push(cur); }
  let dir = 'TD'; const nodes = new Map(), edges = [], groups = [], stack = []; let started = false;
  const node = (id, text, k) => {
    let n = nodes.get(id); if (!n) { n = { id, text: id, k: 'rect' }; nodes.set(id, n); }
    if (text !== undefined) { n.text = label(text); n.k = k; }
    for (const g of stack) if (!g.nodes.includes(id)) g.nodes.push(id);
    return n;
  };
  for (let raw of lines) {
    const line = raw.replace(/%%.*$/, '').trim(); if (!line) continue;
    let m;
    if (!started && (m = /^(flowchart|graph)\b\s*(TB|TD|BT|LR|RL)?/i.exec(line))) { started = true; dir = (m[2] || 'TD').toUpperCase().replace('TB', 'TD'); continue; }
    started = true;
    if (/^(classDef|class|style|linkStyle|click|direction)\b/.test(line)) continue;
    if ((m = /^subgraph\s+(.+)$/.exec(line))) { const s = m[1].trim(); const mm = /^([^\s[]+)\s*\[(.*)\]$/.exec(s); const g = { id: mm ? mm[1] : s.replace(/\s+/g, '_'), title: label(mm ? mm[2] : s), nodes: [] }; groups.push(g); stack.push(g); continue; }
    if (/^end$/.test(line)) { stack.pop(); continue; }
    // утверждение: узлы через связи
    let i = 0; const S = line;
    const ws = () => { while (i < S.length && /\s/.test(S[i])) i++; };
    const parseNodes = () => {
      const list = [];
      for (;;) {
        ws(); const mm = /^[A-Za-z0-9_Ѐ-ӿ][A-Za-z0-9_\-.Ѐ-ӿ]*/.exec(S.slice(i)); if (!mm) break;
        let id = mm[0]; i += id.length; id = id.replace(/\.+$/, m2 => { i -= m2.length; return ''; });
        let text, k;
        for (const [o, c, kind] of BR) {
          if (S.startsWith(o, i)) {
            // кавычки внутри
            let j = i + o.length, q = false;
            for (; j < S.length; j++) { if (S[j] === '"') q = !q; if (!q && S.startsWith(c, j)) break; }
            if (j < S.length) { text = S.slice(i + o.length, j); k = kind; i = j + c.length; break; }
          }
        }
        if (S.startsWith(':::', i)) { const c2 = /^:::[\w-]+/.exec(S.slice(i)); if (c2) i += c2[0].length; }
        list.push(node(id, text, k).id);
        ws(); if (S[i] === '&') { i++; continue; } break;
      }
      return list;
    };
    const parseLink = () => {
      ws(); const rest = S.slice(i);
      let mm = /^(<)?(--|==|-\.)(?![->=.ox])\s*(.+?)\s*(-->|---|==>|===|\.->|\.-|--[ox]|==[ox])/.exec(rest);
      let text = '', tok;
      if (mm) { tok = (mm[1] || '') + mm[2] + mm[4]; text = mm[3]; i += mm[0].length; }
      else { mm = /^(<)?(-\.+->?|-\.+-|={2,}[>ox]?|-{2,}[>ox]?|~~~)/.exec(rest); if (!mm) return null; tok = mm[0]; i += tok.length; }
      ws(); const pm = /^\|([^|]*)\|/.exec(S.slice(i)); if (pm) { text = pm[1]; i += pm[0].length; }
      const end = tok[tok.length - 1];
      return { text: label(text), dash: tok.includes('.'), thick: tok.includes('='), hidden: tok === '~~~', ha: tok[0] === '<' ? 'arrow' : 'none', hb: end === '>' ? 'arrow' : end === 'o' ? 'circle' : end === 'x' ? 'arrow' : 'none' };
    };
    let left = parseNodes(); if (!left.length) continue;
    for (;;) {
      const ln = parseLink(); if (!ln) break;
      const right = parseNodes(); if (!right.length) break;
      if (!ln.hidden) for (const a of left) for (const b of right) edges.push({ a, b, ...ln });
      left = right;
    }
  }
  return { dir, nodes: [...nodes.values()], edges, groups };
}
/** Диаграмма → Mermaid (flowchart). shapes — фигуры, links — связи между ними. */
export function toMermaid(shapes, links, dir = 'TD') {
  const ids = new Map(); let n = 0; const nm = o => { if (!ids.has(o.id)) ids.set(o.id, 'n' + ++n); return ids.get(o.id); };
  const BRK = { rect: ['[', ']'], round: ['(', ')'], terminal: ['([', '])'], predefined: ['[[', ']]'], database: ['[(', ')]'], ellipse: ['((', '))'], hexagon: ['{{', '}}'], trapezoid: ['[/', '\\]'], parallelogram: ['[/', '/]'], diamond: ['{', '}'], callout: ['>', ']'] };
  const q = t => '"' + String(t || '').replace(/"/g, '#quot;').replace(/\n/g, '<br>') + '"';
  const out = ['flowchart ' + dir];
  const conts = shapes.filter(isContainer), plain = shapes.filter(o => !isContainer(o));
  const inside = (c, o) => o.x >= c.x && o.y >= c.y && o.x + o.w <= c.x + c.w && o.y + o.h <= c.y + c.h;
  const done = new Set();
  for (const c of conts) {
    out.push('  subgraph ' + nm(c) + ' [' + q(c.text || SHAPE_NAME[c.k]) + ']');
    for (const o of plain) if (!done.has(o.id) && inside(c, o)) { done.add(o.id); const [a, b] = BRK[o.k] || BRK.rect; out.push('    ' + nm(o) + a + q(o.text) + b); }
    out.push('  end');
  }
  for (const o of plain) if (!done.has(o.id)) { const [a, b] = BRK[o.k] || BRK.rect; out.push('  ' + nm(o) + a + q(o.text) + b); }
  for (const l of links) {
    const a = l.a && l.a.id && ids.get(l.a.id), b = l.b && l.b.id && ids.get(l.b.id); if (!a || !b) continue;
    const dash = l.sd === 'dash' || l.sd === 'dot', thick = (l.sw || 2) >= 4, head = (l.hb || 'arrow') !== 'none', back = (l.ha || 'none') !== 'none';
    let tok = dash ? '-.-' : thick ? '===' : '---'; if (head) tok = dash ? '-.->' : thick ? '==>' : '-->'; if (back && head) tok = '<' + tok;
    out.push('  ' + a + ' ' + tok + (l.text ? '|' + q(l.text) + '|' : '') + ' ' + b);
  }
  return out.join('\n');
}

/**
 * Mermaid → объекты доски (фигуры, связи, рамки для subgraph) от точки (x0, y0).
 * measure(text, fs) — ширина строки текста. Возвращает {objs, dir} или бросает ошибку.
 */
export function buildMermaid(src, x0, y0, opt) {
  const { measure, newId, layer, color, fill, z0 } = opt;
  const g = parseMermaid(src); if (!g.nodes.length) throw new Error('не нашёл ни одного узла — нужен flowchart, например «A[Начало] --> B{Решение}»');
  if (g.nodes.length > 400) throw new Error('слишком много узлов (больше 400)');
  const fs = 18, size = n => {
    const lines = String(n.text || '').split('\n'), tw = Math.max(40, ...lines.map(l => measure(l, fs))), th = lines.length * fs * 1.3;
    let w = Math.max(120, tw + 44), h = Math.max(60, th + 30);
    if (n.k === 'diamond') { w = Math.max(150, tw * 1.6 + 30); h = Math.max(90, th * 2 + 40); }
    else if (n.k === 'ellipse') { w = Math.max(110, tw * 1.35 + 30); h = Math.max(70, th * 1.5 + 24); if (tw < 70) w = h = Math.max(w, h); }
    else if (n.k === 'hexagon' || n.k === 'parallelogram' || n.k === 'trapezoid') w += 40;
    else if (n.k === 'database') h += 24;
    return [Math.round(w), Math.round(h)];
  };
  const dims = new Map(g.nodes.map(n => [n.id, size(n)]));
  const pos = layered(g.nodes.map(n => ({ id: n.id, w: dims.get(n.id)[0], h: dims.get(n.id)[1] })), g.edges.map(e => [e.a, e.b]), g.dir);
  const idOf = new Map(), objs = []; let z = z0;
  // рамки subgraph — под узлами
  const PAD = 30;
  for (const gr of g.groups) {
    if (!gr.nodes.length) continue;
    let b = null; for (const nid of gr.nodes) { const [x, y] = pos.get(nid), [w, h] = dims.get(nid); b = b ? [Math.min(b[0], x), Math.min(b[1], y), Math.max(b[2], x + w), Math.max(b[3], y + h)] : [x, y, x + w, y + h]; }
    objs.push({ id: newId(), t: 'shape', k: 'frame', L: layer, x: x0 + b[0] - PAD, y: y0 + b[1] - PAD - HEAD, w: b[2] - b[0] + PAD * 2, h: b[3] - b[1] + PAD * 2 + HEAD, c: color, f: '', sw: 2, text: gr.title, fs: 16, fb: true, z: z++ });
  }
  for (const n of g.nodes) {
    const [x, y] = pos.get(n.id), [w, h] = dims.get(n.id), id = newId(); idOf.set(n.id, id);
    objs.push({ id, t: 'shape', k: n.k, L: layer, x: x0 + x, y: y0 + y, w, h, c: color, f: fill, sw: 2, text: n.text, fs, z: z++ });
  }
  const horiz = g.dir === 'LR' || g.dir === 'RL';
  for (const e of g.edges) {
    const a = idOf.get(e.a), b = idOf.get(e.b); if (!a || !b) continue;
    const o = { id: newId(), t: 'link', L: layer, a: { id: a }, b: { id: b }, k: a === b ? 'curve' : 'straight', ha: e.ha, hb: e.hb, c: color, sw: e.thick ? 4 : 2, z: z++ };
    if (e.dash) o.sd = 'dash'; if (e.text) { o.text = e.text; o.fs = 15; }
    if (a === b) { o.a.s = horiz ? 't' : 'r'; o.b.s = horiz ? 'r' : 'b'; }
    else { // обратная связь (вверх по схеме) — дугой сбоку, чтобы не резать узлы
      const pa = pos.get(e.a), pb = pos.get(e.b), back = horiz ? (g.dir === 'RL' ? pb[0] > pa[0] : pb[0] < pa[0]) : g.dir === 'BT' ? pb[1] > pa[1] : pb[1] < pa[1];
      if (back) { o.k = 'curve'; o.a.s = o.b.s = horiz ? 'b' : 'r'; }
    }
    objs.push(o);
  }
  return { objs, dir: g.dir };
}

// ───────────── шаблоны ─────────────
export const TEMPLATES = [
  ['flow', 'Блок-схема', 'Начало, шаги, решение, конец', 'flowchart TD\n  s([Начало]) --> a[Собрать данные]\n  a --> d{Всё верно?}\n  d -->|да| b[Обработать]\n  d -->|нет| c[Исправить]\n  c --> a\n  b --> e([Конец])'],
  ['mind', 'Карта мыслей', 'Тема в центре, ветви во все стороны', null],
  ['org', 'Оргструктура', 'Руководитель, отделы, команды', 'flowchart TD\n  ceo[Руководитель] --> a[Разработка]\n  ceo --> b[Дизайн]\n  ceo --> c[Продажи]\n  a --> a1[Бэкенд]\n  a --> a2[Фронтенд]\n  b --> b1[Интерфейсы]\n  c --> c1[Партнёры]\n  c --> c2[Поддержка]'],
  ['er', 'ER-диаграмма', 'Сущности и связи «один ко многим»', null],
  ['lanes', 'Процесс по дорожкам', 'Кто что делает: шаги по ролям', null],
];
/** Шаблон → объекты в точке (x0, y0). */
export function buildTemplate(key, x0, y0, opt) {
  const t = TEMPLATES.find(x => x[0] === key); if (!t) throw new Error('нет шаблона');
  if (t[3]) return buildMermaid(t[3], x0, y0, opt).objs;
  const { newId, layer, color, fill, z0 } = opt; let z = z0; const objs = [];
  const S = (k, x, y, w, h, text, extra = {}) => { const o = { id: newId(), t: 'shape', k, L: layer, x: x0 + x, y: y0 + y, w, h, c: color, f: fill, sw: 2, text, fs: 18, z: z++, ...extra }; objs.push(o); return o; };
  const Lk = (a, b, extra = {}) => { const o = { id: newId(), t: 'link', L: layer, a: { id: a.id }, b: { id: b.id }, k: 'straight', ha: 'none', hb: 'arrow', c: color, sw: 2, z: z++, ...extra }; objs.push(o); return o; };
  if (key === 'mind') {
    const c = S('ellipse', 300, 170, 200, 110, 'Главная тема', { fs: 22, fb: true });
    const br = [['Идея 1', 0, 20], ['Идея 2', 0, 180], ['Идея 3', 0, 340], ['Идея 4', 620, 20], ['Идея 5', 620, 180], ['Идея 6', 620, 340]];
    for (const [tx, x, y] of br) { const n = S('round', x, y, 180, 70, tx); Lk(c, n, { k: 'curve', hb: 'none' }); }
  } else if (key === 'er') {
    const ent = (x, y, title, rows) => S('rect', x, y, 240, 54 + rows.length * 26, title + '\n' + rows.join('\n'), { fa: 'left', va: 'top', fs: 17 });
    const u = ent(0, 0, 'Пользователь', ['id', 'имя', 'почта']);
    const o = ent(400, 0, 'Заказ', ['id', 'пользователь_id', 'дата', 'сумма']);
    const p = ent(800, 0, 'Позиция', ['id', 'заказ_id', 'товар', 'количество']);
    Lk(u, o, { ha: 'one', hb: 'many', a: { id: u.id, s: 'r' }, b: { id: o.id, s: 'l' } });
    Lk(o, p, { ha: 'one', hb: 'many', a: { id: o.id, s: 'r' }, b: { id: p.id, s: 'l' } });
  } else if (key === 'lanes') {
    const W = 1100, H = 190, roles = ['Клиент', 'Менеджер', 'Склад'];
    roles.forEach((r, i) => S('lane', 0, i * H, W, H, r, { f: '', fb: true, fs: 17 }));
    const a = S('terminal', 80, 60, 160, 64, 'Заявка'), b = S('rect', 320, H + 55, 170, 80, 'Проверить заявку'), d = S('diamond', 560, H + 45, 160, 100, 'Есть на складе?');
    const e = S('rect', 800, 2 * H + 55, 170, 80, 'Собрать заказ'), f = S('terminal', 800, 60, 160, 64, 'Получил');
    Lk(a, b, { k: 'elbow' }); Lk(b, d); Lk(d, e, { k: 'elbow', text: 'да', fs: 15 }); Lk(e, f, { k: 'elbow' }); Lk(d, a, { k: 'elbow', text: 'нет', fs: 15, a: { id: d.id, s: 't' }, b: { id: a.id, s: 'b' } });
  }
  return objs;
}
