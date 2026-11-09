/*
 * Voice core animation engine: one instance per core, a seeded random source so a run can be
 * repeated frame for frame, a fixed 60 Hz step, and live features pushed from outside.
 */
import { FakeSpeaker, type SpeechFrame, synthBands } from './speech';

export type CoreState = 'idle' | 'listening' | 'thinking' | 'speaking';

/** Live audio features. `rms` is loudness normalised to 0..1, `pitch` is Hz (0 when unvoiced), `bands` has 48 values in 0..1. */
export interface Features {
  rms: number;
  pitch: number;
  bands: ArrayLike<number>;
}

export const TAU = Math.PI * 2;
export const NB = 48;
export const STEP = 1 / 60;
const MAX_CATCH_UP = 0.05;
const HN = 240; // loudness history samples (about 4 s at 60 Hz)
const LIVE_HOLD = 0.25; // live features older than this fall back to silence or the demo voice

export const clamp = (v: number, a: number, b: number) => (v < a ? a : v > b ? b : v);
export const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);

/** mulberry32: small, fast, seedable PRNG returning floats in [0, 1). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface Track {
  E: number;
  p: number;
  bands: Float32Array;
  raw: Float32Array;
  onset: number;
  fric: number;
  active: number;
  hist: Float32Array;
  hi: number;
}
const makeTrack = (): Track => ({
  E: 0,
  p: 0.4,
  bands: new Float32Array(NB),
  raw: new Float32Array(NB),
  onset: 0,
  fric: 0,
  active: 0,
  hist: new Float32Array(HN),
  hi: 0,
});

export interface Pulse {
  born: number;
  a: number;
  s: number;
}

export interface EngineOptions {
  seed?: number;
  /** Drive the voice from the built-in fake speech generator when no live features arrive. */
  demo?: boolean;
  /** Halve the animation speed and stop the idle glint. */
  reducedMotion?: boolean;
}

type Who = 'ai' | 'user';
const SILENT: SpeechFrame = { E: 0, f0: 0, F1: 500, F2: 1500, fric: 0, voiced: 0, onset: 0 };

export class CoreEngine {
  readonly rnd: (a: number, b: number) => number;
  readonly random: () => number;
  readonly S = { state: 'speaking' as CoreState, idle: 0, listen: 0, think: 0, speak: 0, t: 0 };
  readonly TH = {
    pulses: [] as Pulse[],
    next: 0,
    step: 0,
    nextStep: 0,
    spin: 0,
    spinV: 0.3,
    arc: 0,
    arcV: 0,
    burst: 0,
  };
  readonly ID = { breath: 0, glint: -1, nextGlint: 2 };
  readonly T: Record<Who, Track> = { ai: makeTrack(), user: makeTrack() };
  readonly SP: Record<Who, FakeSpeaker>;
  /** Particle shell memory: three layers of [angle, jitter, tint]. */
  readonly MEM: Float64Array[];
  demo: boolean;
  reducedMotion: boolean;
  private acc = 0;
  private live: Features | null = null;
  private liveAt = -1;
  private readonly frame: SpeechFrame = { ...SILENT };

  constructor(opts: EngineOptions = {}) {
    this.random = mulberry32(opts.seed ?? 1);
    const r = this.random;
    this.rnd = (a, b) => a + r() * (b - a);
    this.demo = opts.demo ?? false;
    this.reducedMotion = opts.reducedMotion ?? false;
    // Draw order: both speakers, then the particle memory.
    this.SP = { ai: new FakeSpeaker('ai', this.rnd, r), user: new FakeSpeaker('user', this.rnd, r) };
    this.MEM = [0, 1, 2].map(() => {
      const m = new Float64Array(560 * 3);
      for (let i = 0; i < 560; i++) {
        m[i * 3] = r();
        m[i * 3 + 1] = r() - 0.5;
        m[i * 3 + 2] = r();
      }
      return m;
    });
  }

  setState(s: CoreState) {
    this.S.state = s;
  }

  /** Live features for whoever is talking now: the user while listening, Markon while speaking. */
  pushFeatures(f: Features) {
    this.live = f;
    this.liveAt = this.S.t;
  }

  /** Advance by wall-clock seconds using a fixed 60 Hz step. Returns the number of steps taken. */
  advance(dt: number): number {
    this.acc += Math.min(MAX_CATCH_UP, dt * (this.reducedMotion ? 0.5 : 1));
    let n = 0;
    while (this.acc >= STEP - 1e-9) {
      this.step(STEP);
      this.acc -= STEP;
      n++;
    }
    return n;
  }

  private liveFrame(tr: Track, f: Features): SpeechFrame {
    const o = this.frame;
    o.E = clamp(f.rms, 0, 1);
    o.f0 = f.pitch > 0 ? f.pitch : 0;
    o.F1 = 500;
    o.F2 = 1500;
    o.voiced = o.f0 ? 1 : 0;
    o.onset = 0;
    let hiE = 0;
    for (let k = 0; k < NB; k++) {
      const v = clamp(f.bands[k] ?? 0, 0, 1);
      tr.raw[k] = v;
      if (k >= 38) hiE += v;
    }
    o.fric = hiE / 10 > 0.35 ? 1 : 0;
    return o;
  }

  step(dt: number) {
    const { S, TH, ID, T, SP } = this;
    S.t += dt;
    const w = (k: 'idle' | 'listen' | 'think' | 'speak', on: boolean) => {
      S[k] += ((on ? 1 : 0) - S[k]) * 0.045;
    };
    w('idle', S.state === 'idle');
    w('listen', S.state === 'listening');
    w('think', S.state === 'thinking');
    w('speak', S.state === 'speaking');
    const liveFresh = this.live !== null && S.t - this.liveAt <= LIVE_HOLD;
    for (const who of ['ai', 'user'] as const) {
      const on = (who === 'ai' && S.state === 'speaking') || (who === 'user' && S.state === 'listening');
      let o: SpeechFrame;
      if (on && liveFresh && this.live) o = this.liveFrame(T[who], this.live);
      else {
        o = on && this.demo ? SP[who].step(dt) : SILENT;
        synthBands(o, T[who].raw);
      }
      feed(T[who], o, on ? 1 : 0);
    }
    ID.breath = 0.5 + 0.5 * Math.sin((S.t * TAU) / 5.5);
    if (S.t > ID.nextGlint) {
      ID.glint = this.reducedMotion ? -1 : S.t;
      ID.nextGlint = S.t + this.rnd(5.5, 9);
    }
    if (S.think > 0.05) {
      if (S.t > TH.next) {
        const st = this.rnd(0.45, 1);
        TH.pulses.push({ born: S.t, a: this.rnd(0, TAU), s: st });
        TH.burst = Math.max(TH.burst, st);
        TH.next = S.t + this.rnd(0.45, 1.5);
      }
      if (S.t > TH.nextStep) {
        TH.step = (TH.step + (this.random() < 0.2 ? 2 : 1)) % 48;
        TH.nextStep = S.t + this.rnd(0.05, 0.16) * (1.3 - TH.burst * 0.5);
      }
    }
    if (TH.pulses.length && S.t - (TH.pulses[0] as Pulse).born >= 1.6) {
      TH.pulses = TH.pulses.filter((p) => S.t - p.born < 1.6);
    }
    TH.burst *= 0.985;
    TH.spinV += (0.25 + S.think * (0.9 + TH.burst * 2.2) - TH.spinV) * 0.04;
    TH.spin += TH.spinV * dt;
    const arcTarget = S.think * (1.2 + TH.burst * 2.6);
    TH.arcV += (arcTarget - TH.arcV) * 0.05;
    TH.arc += TH.arcV * dt;
  }
}

function feed(tr: Track, o: SpeechFrame, live: number) {
  tr.E += (o.E - tr.E) * (o.E > tr.E ? 0.45 : 0.16);
  if (o.voiced > 0.5 && o.f0 > 0) {
    const p = clamp(Math.log(o.f0 / 80) / Math.log(300 / 80), 0, 1);
    tr.p += (p - tr.p) * 0.25;
  }
  for (let k = 0; k < NB; k++) {
    const v = (tr.raw[k] as number) * live;
    const b = tr.bands[k] as number;
    tr.bands[k] = b + (v - b) * (v > b ? 0.42 : 0.13);
  }
  tr.onset = Math.max(tr.onset * 0.88, o.onset * o.E);
  tr.fric += (o.fric - tr.fric) * 0.3;
  tr.active += ((tr.E > 0.05 ? 1 : 0) - tr.active) * (tr.E > 0.05 ? 0.2 : 0.03);
  tr.hi = (tr.hi + 1) % HN;
  tr.hist[tr.hi] = tr.E;
}

export function histAt(tr: Track, d: number) {
  const k = clamp(d, 0, 0.999) * (HN - 1);
  const i = k | 0;
  const f = k - i;
  const a = tr.hist[(tr.hi - i + HN) % HN] as number;
  const b = tr.hist[(tr.hi - i - 1 + HN) % HN] as number;
  return a + (b - a) * f;
}
