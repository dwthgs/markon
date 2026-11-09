/*
 * Drawing for the three variants. Every call is drawn
 * centred on (0, 0) with radius R; drawVariant sets up the transform.
 */
import { type CoreEngine, clamp, ease, histAt, NB, TAU, type Track } from './engine';

export type Variant = 'prime' | 'shell' | 'minimal';
export const VARIANTS: readonly Variant[] = ['prime', 'shell', 'minimal'];

type RGB = readonly [number, number, number];
type Ctx = CanvasRenderingContext2D;
const CY: RGB = [0, 210, 235];
const BL: RGB = [0, 110, 200];
const TE: RGB = [60, 230, 230];
const AM: RGB = [255, 176, 40];
const WH: RGB = [228, 246, 252];
const IC: RGB = [170, 240, 255];
const rgba = (c: RGB, a: number) => `rgba(${c[0] | 0},${c[1] | 0},${c[2] | 0},${a < 0 ? 0 : a > 1 ? 1 : a})`;
const mix = (a: RGB, b: RGB, t: number): RGB => [
  a[0] + (b[0] - a[0]) * t,
  a[1] + (b[1] - a[1]) * t,
  a[2] + (b[2] - a[2]) * t,
];

const fromTop = (a: number) => {
  let d = (((a + Math.PI / 2) % TAU) + TAU) % TAU;
  if (d > Math.PI) d = TAU - d;
  return d / Math.PI;
};
const clockU = (a: number) => ((((a + Math.PI / 2) % TAU) + TAU) % TAU) / TAU;

function glow(c: Ctx, r: number, col: RGB, a: number) {
  if (a <= 0) return;
  const g = c.createRadialGradient(0, 0, 0, 0, 0, r);
  g.addColorStop(0, rgba(col, a));
  g.addColorStop(0.5, rgba(col, a * 0.25));
  g.addColorStop(1, rgba(col, 0));
  c.fillStyle = g;
  c.beginPath();
  c.arc(0, 0, r, 0, TAU);
  c.fill();
}
function arc(c: Ctx, r: number, a0: number, a1: number, w: number, col: RGB, a: number, blur?: number) {
  c.beginPath();
  c.arc(0, 0, r, a0, a1);
  c.lineWidth = w;
  c.strokeStyle = rgba(col, a);
  if (blur) {
    c.shadowColor = rgba(col, 0.85);
    c.shadowBlur = blur;
  }
  c.stroke();
  c.shadowBlur = 0;
}
function cellPath(c: Ctx, r0: number, r1: number, a0: number, a1: number) {
  c.beginPath();
  c.arc(0, 0, r1, a0, a1);
  c.arc(0, 0, r0, a1, a0, true);
  c.closePath();
}
const bandAt = (tr: Track, u: number) => {
  const x = clamp(u, 0, 1) * (NB - 1);
  const i = x | 0;
  const f = x - i;
  return (tr.bands[i] as number) * (1 - f) + (tr.bands[Math.min(NB - 1, i + 1)] as number) * f;
};
function glintAt(e: CoreEngine, a: number) {
  if (e.ID.glint < 0) return 0;
  const age = e.S.t - e.ID.glint;
  if (age > 1.8) return 0;
  const head = -Math.PI / 2 + ease(age / 1.8) * TAU;
  const d = (((a - head) % TAU) + TAU) % TAU;
  return Math.max(0, 1 - d / 0.5) * (1 - age / 1.8);
}

function cellValue(e: CoreEngine, a: number) {
  const { S, TH, ID } = e;
  const s = e.T.ai;
  const l = e.T.user;
  const top = fromTop(a);
  const u = clockU(a);
  let v = 0.1 + 0.07 * ID.breath * S.idle + glintAt(e, a) * 0.8 * S.idle;
  v += S.speak * bandAt(s, top) * 1.15;
  v += S.listen * bandAt(l, 1 - top) * 0.95;
  if (S.think > 0.02) {
    const step = TH.step / 48;
    const dd = (((step - u) % 1) + 1) % 1;
    v += S.think * Math.max(0, 1 - dd * 6) ** 2 * 0.9;
    for (const p of TH.pulses) {
      const age = (S.t - p.born) / 1.6;
      const da = Math.abs(((((a - p.a) % TAU) + TAU + Math.PI) % TAU) - Math.PI);
      v += S.think * p.s * Math.max(0, 1 - da / 0.35) * Math.max(0, 1 - age * 2) * 0.8;
    }
  }
  return clamp(v, 0, 1);
}
const cellColor = (e: CoreEngine, v: number) =>
  e.S.listen > 0.5 ? mix([20, 90, 120], IC, v) : mix(BL, mix(CY, WH, 0.25), v);

/** Tick ring: loudness history scrolling clockwise from the top, so words and pauses pass by. */
function tickTrace(e: CoreEngine, c: Ctx, R: number, r0: number, len: number, N: number, w: number) {
  const { S, TH, ID } = e;
  const s = e.T.ai;
  const l = e.T.user;
  for (let i = 0; i < N; i++) {
    const a = (i / N) * TAU - Math.PI / 2;
    const d = i / N;
    let v = S.speak * histAt(s, d) + S.listen * histAt(l, d) * 0.9;
    v += (0.08 + 0.06 * ID.breath) * S.idle + glintAt(e, a) * 0.7 * S.idle;
    if (S.think > 0.02) {
      const k = Math.floor(TH.spin * 12) % N;
      v += S.think * (i % 6 === k % 6 ? 0.35 : 0.05);
    }
    v = clamp(v, 0, 1);
    const r1 = r0 + len * (0.35 + v * 0.65);
    c.strokeStyle = rgba(mix(CY, WH, v * 0.5), 0.18 + v * 0.8);
    c.lineWidth = R * w;
    c.beginPath();
    c.moveTo(Math.cos(a) * r0, Math.sin(a) * r0);
    c.lineTo(Math.cos(a) * r1, Math.sin(a) * r1);
    c.stroke();
  }
}
function bandRing(e: CoreEngine, c: Ctx, R: number, r0: number, r1: number, N: number, gap: number) {
  for (let i = 0; i < N; i++) {
    const a0 = (i / N) * TAU - Math.PI / 2 + gap;
    const a1 = ((i + 1) / N) * TAU - Math.PI / 2 - gap;
    const v = cellValue(e, (a0 + a1) / 2);
    cellPath(c, R * r0, R * r1, a0, a1);
    c.fillStyle = rgba(cellColor(e, v), 0.07 + v * 0.9);
    c.fill();
  }
  arc(c, R * (r1 + 0.006), 0, TAU, R * 0.003, CY, 0.4);
  arc(c, R * (r0 - 0.006), 0, TAU, R * 0.003, CY, 0.4);
}
function amberArc(e: CoreEngine, c: Ctx, R: number, r: number) {
  const { S, TH } = e;
  const park = Math.PI * 0.8;
  const a = park + TH.arc;
  const span = 0.32 + S.think * (0.3 + TH.burst * 0.25);
  arc(c, R * r, a - span, a + span, R * (0.01 + S.think * 0.006), AM, 0.45 + S.think * 0.55, R * 0.03);
}
/** Thinking: information flowing inward. */
function thinkPulses(e: CoreEngine, c: Ctx, R: number, rFrom: number, rTo: number) {
  const { S, TH } = e;
  for (const p of TH.pulses) {
    const age = (S.t - p.born) / 1.6;
    if (age > 1) continue;
    const r = R * (rFrom + (rTo - rFrom) * ease(age));
    c.fillStyle = rgba(mix(AM, WH, 0.3), S.think * (1 - age) * p.s);
    c.beginPath();
    c.arc(Math.cos(p.a) * r, Math.sin(p.a) * r, R * 0.012 * (1 - age * 0.5), 0, TAU);
    c.fill();
  }
}

/* geodesic sphere: icosahedron subdivided twice */
type V3 = [number, number, number];
const PHI = (1 + Math.sqrt(5)) / 2;
const GV: V3[] = (
  [
    [-1, PHI, 0],
    [1, PHI, 0],
    [-1, -PHI, 0],
    [1, -PHI, 0],
    [0, -1, PHI],
    [0, 1, PHI],
    [0, -1, -PHI],
    [0, 1, -PHI],
    [PHI, 0, -1],
    [PHI, 0, 1],
    [-PHI, 0, -1],
    [-PHI, 0, 1],
  ] as V3[]
).map((v) => {
  const q = Math.hypot(...v);
  return v.map((x) => x / q) as V3;
});
let GF: V3[] = [
  [0, 11, 5],
  [0, 5, 1],
  [0, 1, 7],
  [0, 7, 10],
  [0, 10, 11],
  [1, 5, 9],
  [5, 11, 4],
  [11, 10, 2],
  [10, 7, 6],
  [7, 1, 8],
  [3, 9, 4],
  [3, 4, 2],
  [3, 2, 6],
  [3, 6, 8],
  [3, 8, 9],
  [4, 9, 5],
  [2, 4, 11],
  [6, 2, 10],
  [8, 6, 7],
  [9, 8, 1],
];
for (let s = 0; s < 2; s++) {
  const cache = new Map<string, number>();
  const nf: V3[] = [];
  const mid = (a: number, b: number) => {
    const k = a < b ? `${a}_${b}` : `${b}_${a}`;
    const hit = cache.get(k);
    if (hit !== undefined) return hit;
    const A = GV[a] as V3;
    const B = GV[b] as V3;
    const m = A.map((x, i) => (x + (B[i] as number)) / 2) as V3;
    const q = Math.hypot(...m);
    GV.push(m.map((x) => x / q) as V3);
    cache.set(k, GV.length - 1);
    return GV.length - 1;
  };
  for (const [a, b, c] of GF) {
    const ab = mid(a, b);
    const bc = mid(b, c);
    const ca = mid(c, a);
    nf.push([a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]);
  }
  GF = nf;
}
const GE: [number, number][] = [];
{
  const seen = new Set<string>();
  for (const f of GF)
    for (let i = 0; i < 3; i++) {
      const a = f[i] as number;
      const b = f[(i + 1) % 3] as number;
      const k = a < b ? `${a}_${b}` : `${b}_${a}`;
      if (!seen.has(k)) {
        seen.add(k);
        GE.push([a, b]);
      }
    }
}
/** Projected sphere vertices, reused every frame: x, y, z per vertex. */
const PROJ = new Float64Array(GV.length * 3);

function sphere(e: CoreEngine, c: Ctx, R: number, r: number) {
  const { S, TH, ID } = e;
  const s = e.T.ai;
  const l = e.T.user;
  const yaw = S.t * 0.22 + TH.spin * 1.6;
  const pitch = -0.32 + 0.05 * Math.sin(S.t * 0.25);
  const sc = r * (1 + 0.012 * ID.breath * S.idle + S.speak * s.E * 0.06 - S.listen * l.E * 0.05);
  const cy = Math.cos(yaw);
  const sy = Math.sin(yaw);
  const cp = Math.cos(pitch);
  const sp = Math.sin(pitch);
  for (let i = 0; i < GV.length; i++) {
    const v = GV[i] as V3;
    const x = v[0] * cy + v[2] * sy;
    const z0 = -v[0] * sy + v[2] * cy;
    const y = v[1] * cp - z0 * sp;
    const z = v[1] * sp + z0 * cp;
    PROJ[i * 3] = x * sc;
    PROJ[i * 3 + 1] = y * sc;
    PROJ[i * 3 + 2] = z;
  }
  const scan = S.think > 0.02 ? Math.sin(TH.spin * 3) * sc : 9e9;
  const pitchTint = S.speak * s.p;
  c.lineWidth = R * 0.0035;
  for (const [a, b] of GE) {
    const px = PROJ[a * 3] as number;
    const py = PROJ[a * 3 + 1] as number;
    const pz = PROJ[a * 3 + 2] as number;
    const qx = PROJ[b * 3] as number;
    const qy = PROJ[b * 3 + 1] as number;
    const qz = PROJ[b * 3 + 2] as number;
    const f = ((pz + qz) / 2 + 1) / 2;
    // speaking: edges light up by the spectrum at their latitude, so pitch and vowels show as bands of light
    const lat = 1 - (((GV[a] as V3)[1] + (GV[b] as V3)[1]) / 2 + 1) / 2;
    const spec = S.speak * bandAt(s, lat) * 1.2 + S.listen * bandAt(l, 1 - lat) * 0.9;
    const nearScan = Math.max(0, 1 - Math.abs((py + qy) / 2 - scan) / (sc * 0.12)) * S.think;
    c.strokeStyle = rgba(
      mix(TE, WH, f * 0.5 + pitchTint * 0.3 + nearScan * 0.4),
      0.05 + f * (0.4 + spec * 0.55) + nearScan * 0.6 + glintAt(e, Math.atan2(py, px)) * 0.4 * S.idle,
    );
    c.beginPath();
    c.moveTo(px, py);
    c.lineTo(qx, qy);
    c.stroke();
  }
  arc(c, sc * 1.05, 0, TAU, R * 0.008, WH, 0.85, R * 0.02);
}

/** Particle shell: takes the shape of the spectrum while speaking, pulls inward while listening. */
function shell(e: CoreEngine, c: Ctx, R: number, r0: number, layers: number, spread: number) {
  const { S, TH, ID } = e;
  const s = e.T.ai;
  const l = e.T.user;
  for (let li = 0; li < layers; li++) {
    const pts = e.MEM[li % 3] as Float64Array;
    for (let i = 0; i < pts.length; i += 3) {
      const u = pts[i] as number;
      const j = pts[i + 1] as number;
      const k = pts[i + 2] as number;
      const a = u * TAU - Math.PI / 2 + S.t * 0.03 * (li % 2 ? 1 : -1) + TH.spin * 0.6 * (li % 2 ? 1 : -1);
      const top = fromTop(a);
      const sp = S.speak * (bandAt(s, top) * 0.16 + s.onset * 0.03);
      const ls = S.listen * (-bandAt(l, 1 - top) * 0.1 - l.E * 0.03);
      const th = -S.think * (0.03 + TH.burst * 0.04) * Math.max(0, Math.sin(a * 3 + TH.spin * 4));
      const id = 0.012 * Math.sin(a * 3 + S.t * 0.5 + li * 2) * (0.5 + ID.breath * 0.5);
      const rr = R * (r0 + li * 0.024 + j * spread + sp + ls + th + id);
      c.fillStyle = rgba(mix(TE, WH, k * 0.5), (0.3 + k * 0.55) * (0.7 + S.speak * s.E * 0.5));
      c.fillRect(Math.cos(a) * rr, Math.sin(a) * rr, R * 0.0055, R * 0.0055);
    }
  }
}

function specValue(e: CoreEngine, a: number) {
  const { S, TH, ID } = e;
  const s = e.T.ai;
  const l = e.T.user;
  const top = fromTop(a);
  let v =
    S.speak * bandAt(s, top) * 1.2 +
    S.listen * bandAt(l, 1 - top) * 0.95 +
    (0.05 + 0.05 * ID.breath) * S.idle +
    glintAt(e, a) * 0.6 * S.idle;
  if (S.think > 0.02) {
    const step = TH.step / 48;
    const dd = (((step - clockU(a)) % 1) + 1) % 1;
    v += S.think * Math.max(0, 1 - dd * 5) ** 2 * 0.75;
    for (const p of TH.pulses) {
      const age = (S.t - p.born) / 1.6;
      const da = Math.abs(((((a - p.a) % TAU) + TAU + Math.PI) % TAU) - Math.PI);
      v += S.think * p.s * Math.max(0, 1 - da / 0.3) * Math.max(0, 1 - age * 2) * 0.6;
    }
  }
  return clamp(v, 0, 1);
}
const barCol = (e: CoreEngine, v: number) =>
  e.S.listen > 0.5 ? mix([40, 120, 140], IC, v) : mix(BL, mix(CY, WH, 0.3), v);

/** Radial spectrum bars pointing outward. */
function specBars(e: CoreEngine, c: Ctx, R: number, r0: number, len: number, N: number, w: number) {
  c.lineCap = 'round';
  for (let i = 0; i < N; i++) {
    const a = (i / N) * TAU - Math.PI / 2;
    const v = specValue(e, a);
    const pull = e.S.listen * v * 0.05;
    const ra = R * (r0 - pull);
    const rb = R * (r0 + len * (0.08 + v) - pull);
    c.strokeStyle = rgba(barCol(e, v), 0.22 + v * 0.78);
    c.lineWidth = R * w;
    c.beginPath();
    c.moveTo(Math.cos(a) * ra, Math.sin(a) * ra);
    c.lineTo(Math.cos(a) * rb, Math.sin(a) * rb);
    c.stroke();
  }
  c.lineCap = 'butt';
}
function disc(e: CoreEngine, c: Ctx, R: number, r: number) {
  const ev = e.S.speak * e.T.ai.E;
  const li = e.S.listen * e.T.user.E;
  glow(
    c,
    R * (r + ev * 0.07 - li * 0.03 + e.ID.breath * 0.01 * e.S.idle),
    CY,
    0.35 + ev * 0.45 + e.S.think * e.TH.burst * 0.2,
  );
  arc(c, R * (r - li * 0.02), 0, TAU, R * 0.006, WH, 0.6 + ev * 0.4, R * 0.03);
}

const DRAW: Record<Variant, (e: CoreEngine, c: Ctx, R: number) => void> = {
  prime: (e, c, R) => {
    shell(e, c, R, 0.66, 4, 0.026);
    bandRing(e, c, R, 0.42, 0.52, 56, 0.01);
    thinkPulses(e, c, R, 0.66, 0.36);
    glow(c, R * 0.38, TE, 0.08 + e.S.speak * e.T.ai.E * 0.14);
    sphere(e, c, R, R * 0.33);
  },
  shell: (e, c, R) => {
    shell(e, c, R, 0.78, 3, 0.022);
    specBars(e, c, R, 0.5, 0.22, 96, 0.012);
    tickTrace(e, c, R, R * 0.36, R * 0.06, 72, 0.004);
    amberArc(e, c, R, 0.46);
    thinkPulses(e, c, R, 0.76, 0.2);
    disc(e, c, R, 0.3);
  },
  minimal: (e, c, R) => {
    specBars(e, c, R, 0.46, 0.26, 72, 0.016);
    thinkPulses(e, c, R, 0.76, 0.28);
    glow(c, R * 0.34, TE, 0.1 + e.S.speak * e.T.ai.E * 0.16);
    sphere(e, c, R, R * 0.25);
  },
};

/** Clear a square canvas of `W` device pixels and draw one variant centred in it. */
export function drawVariant(e: CoreEngine, c: Ctx, W: number, variant: Variant) {
  const R = W / 2;
  c.setTransform(1, 0, 0, 1, 0, 0);
  c.clearRect(0, 0, W, W);
  c.translate(R, R);
  c.lineCap = 'butt';
  DRAW[variant](e, c, R);
}
