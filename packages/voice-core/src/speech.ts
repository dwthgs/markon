/*
 * Fake speech: phrases → words → syllables with rate, pitch contour, stress, questions, vowel
 * formants and fricative bursts. Used as the demo feature source.
 */
const NB = 48;
const FLO = 90;
const FHI = 7000;
/** Centre frequency of each of the 48 log-spaced bands, 90 Hz to 7 kHz. */
export const BAND_HZ = Array.from({ length: NB }, (_, k) => FLO * (FHI / FLO) ** (k / (NB - 1)));
const VOWELS: ReadonlyArray<readonly [number, number]> = [
  [730, 1090],
  [270, 2290],
  [300, 870],
  [530, 1840],
  [570, 840],
  [440, 1020],
  [660, 1720],
  [390, 1990],
];
const g2 = (f: number, F: number, w: number) => {
  const x = Math.log2(f / F) / w;
  return Math.exp(-x * x);
};
const clamp = (v: number, a: number, b: number) => (v < a ? a : v > b ? b : v);

export interface SpeechFrame {
  E: number;
  f0: number;
  F1: number;
  F2: number;
  fric: number;
  voiced: number;
  onset: number;
}

interface Syllable {
  start: number;
  dur: number;
  stress: boolean;
  F1: number;
  F2: number;
  fric: number;
  last: boolean;
  k: number;
}

export class FakeSpeaker {
  private t = 0;
  private mode: 'pause' | 'gap' | 'syl' = 'pause';
  private until: number;
  private syl: Syllable | null = null;
  private rate = 1;
  private words = 0;
  private f0base = 0;
  private question = false;
  private sylDone = 0;
  private wordSyl = 0;
  private readonly o: SpeechFrame = { E: 0, f0: 0, F1: 500, F2: 1500, fric: 0, voiced: 0, onset: 0 };

  constructor(
    private readonly s: 'ai' | 'user',
    private readonly rnd: (a: number, b: number) => number,
    private readonly random: () => number,
  ) {
    this.until = rnd(0.3, 0.8);
    this.newPhrase();
  }

  private newPhrase() {
    const ai = this.s === 'ai';
    const rnd = this.rnd;
    this.rate = rnd(ai ? 0.85 : 0.8, ai ? 1.25 : 1.35);
    this.words = Math.floor(rnd(ai ? 4 : 2, ai ? 10 : 7));
    this.f0base = (ai ? 112 : 122) * rnd(0.92, 1.12);
    this.question = this.random() < (ai ? 0.18 : 0.25);
    this.sylDone = 0;
    this.wordSyl = 0;
  }

  private nextSyl() {
    const rnd = this.rnd;
    if (this.wordSyl <= 0) {
      if (this.words <= 0) {
        const ai = this.s === 'ai';
        this.mode = 'pause';
        this.until = this.t + (ai ? rnd(0.35, 0.9) : this.random() < 0.15 ? rnd(1.8, 3.2) : rnd(0.4, 1.2));
        this.newPhrase();
        return;
      }
      this.words--;
      this.wordSyl = Math.floor(rnd(1, 3.6));
      if (this.sylDone > 0) {
        this.mode = 'gap';
        this.until = this.t + rnd(0.03, 0.13) / this.rate;
        return;
      }
    }
    this.wordSyl--;
    const dur = rnd(0.12, 0.26) / this.rate;
    const v = VOWELS[(this.random() * VOWELS.length) | 0] as readonly [number, number];
    const last = this.words === 0 && this.wordSyl === 0;
    const stress = this.random() < 0.35;
    const F1 = v[0] * rnd(0.9, 1.1);
    const F2 = v[1] * rnd(0.92, 1.08);
    const fric = this.random() < 0.32 ? rnd(0.03, 0.07) : 0;
    this.syl = { start: this.t, dur, stress, F1, F2, fric, last, k: this.sylDone };
    this.sylDone++;
    this.mode = 'syl';
    this.until = this.t + dur;
  }

  step(dt: number): SpeechFrame {
    this.t += dt;
    if (this.t >= this.until) this.nextSyl();
    const o = this.o;
    o.E = 0;
    o.f0 = 0;
    o.F1 = 500;
    o.F2 = 1500;
    o.fric = 0;
    o.voiced = 0;
    o.onset = 0;
    if (this.mode !== 'syl' || !this.syl) return o;
    const s = this.syl;
    const x = this.t - s.start;
    const d = s.dur;
    const env = Math.min(1, x / 0.028) * Math.max(0, 1 - (x / d) ** 2.6);
    const decl = 1 - 0.16 * Math.min(1, s.k / 12);
    let f0 = this.f0base * decl * (s.stress ? 1.14 : 1) * (1 + 0.025 * Math.sin(this.t * 31));
    if (s.last && this.question) f0 *= 1 + 0.35 * (x / d);
    if (s.last && !this.question) f0 *= 1 - 0.1 * (x / d);
    const inFric = s.fric && x < s.fric;
    o.E = (s.stress ? 1 : 0.72) * decl * env;
    o.f0 = f0;
    o.F1 = s.F1;
    o.F2 = s.F2;
    o.fric = inFric ? 0.85 : 0;
    o.voiced = inFric ? 0.25 : 1;
    o.onset = x < 0.04 ? 1 : 0;
    return o;
  }
}

/** Turn one speech frame into 48 band energies (0..1). Writes into `out`, no allocation. */
export function synthBands(o: SpeechFrame, out: Float32Array) {
  for (let k = 0; k < NB; k++) {
    const f = BAND_HZ[k] as number;
    const tilt = Math.min(1.25, (f / 220) ** -0.55);
    const harm = o.f0 > 0 && f < 1300 ? 0.35 + 0.65 * Math.abs(Math.cos((Math.PI * f) / o.f0)) ** 6 : 1;
    const form = 0.18 + g2(f, o.F1, 0.22) + 0.85 * g2(f, o.F2, 0.18) + 0.45 * g2(f, 2600, 0.14);
    let v = o.voiced * tilt * harm * form;
    v += o.fric * 1.2 * g2(f, 4800, 0.6);
    out[k] = v;
  }
  let mx = 1e-6;
  for (let k = 0; k < NB; k++) mx = Math.max(mx, out[k] as number);
  for (let k = 0; k < NB; k++)
    out[k] = o.E > 0 ? clamp(o.E * ((out[k] as number) / mx) ** 0.75 * 1.05, 0, 1) : 0;
}
