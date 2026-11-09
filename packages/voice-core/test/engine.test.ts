import { describe, expect, it } from 'vitest';
import { drawVariant, VARIANTS } from '../src/draw';
import { CoreEngine, type CoreState, NB, STEP } from '../src/engine';
import { recordingContext } from './recording-context';

const steps = (e: CoreEngine, n: number) => {
  for (let i = 0; i < n; i++) e.step(STEP);
};
const weights = (e: CoreEngine) => [e.S.idle, e.S.listen, e.S.think, e.S.speak];
const silence = { rms: 0, pitch: 0, bands: new Float32Array(NB) };

describe('state blending', () => {
  it('blends smoothly from one state to the next, with no jumps', () => {
    const e = new CoreEngine({ seed: 1, demo: true });
    e.setState('idle');
    steps(e, 240);
    expect(e.S.idle).toBeGreaterThan(0.99);

    e.setState('speaking');
    let prev = e.S.speak;
    for (let i = 0; i < 60; i++) {
      e.step(STEP);
      expect(e.S.speak).toBeGreaterThan(prev);
      expect(e.S.speak - prev).toBeLessThan(0.05);
      prev = e.S.speak;
    }
    // half-way through a cross-fade both states are visible
    expect(e.S.idle).toBeGreaterThan(0.05);
    expect(e.S.speak).toBeGreaterThan(0.5);
    steps(e, 240);
    expect(e.S.speak).toBeGreaterThan(0.99);
    expect(e.S.idle).toBeLessThan(0.01);
  });

  it('keeps the weights summing to at most one', () => {
    const e = new CoreEngine({ seed: 2, demo: true });
    const order: CoreState[] = ['idle', 'listening', 'thinking', 'speaking', 'idle'];
    for (const s of order) {
      e.setState(s);
      for (let i = 0; i < 50; i++) {
        e.step(STEP);
        const sum = weights(e).reduce((a, b) => a + b, 0);
        expect(sum).toBeLessThanOrEqual(1.000001);
      }
    }
  });
});

describe('listening', () => {
  it('follows live features and settles when the user goes quiet', () => {
    const e = new CoreEngine({ seed: 3, demo: false });
    e.setState('listening');
    const loud = { rms: 0.8, pitch: 140, bands: new Float32Array(NB).fill(0.7) };
    for (let i = 0; i < 60; i++) {
      e.pushFeatures(loud);
      e.step(STEP);
    }
    expect(e.T.user.E).toBeGreaterThan(0.5);
    expect(Math.max(...e.T.user.bands)).toBeGreaterThan(0.5);

    for (let i = 0; i < 60; i++) {
      e.pushFeatures(silence);
      e.step(STEP);
    }
    expect(e.T.user.E).toBeLessThan(0.01);
    expect(Math.max(...e.T.user.bands)).toBeLessThan(0.01);
  });

  it('stays still in silence when no demo source is attached', () => {
    const e = new CoreEngine({ seed: 4, demo: false });
    e.setState('listening');
    steps(e, 120);
    expect(e.T.user.E).toBe(0);
    expect(Math.max(...e.T.user.bands)).toBe(0);
  });
});

describe('determinism', () => {
  const run = (seed: number) => {
    const e = new CoreEngine({ seed, demo: true });
    const ctx = recordingContext();
    const script: CoreState[] = ['speaking', 'thinking', 'listening', 'idle'];
    for (const s of script) {
      e.setState(s);
      for (let i = 0; i < 90; i++) {
        e.step(STEP);
        if (i % 30 === 0) for (const v of VARIANTS) drawVariant(e, ctx.ctx, 400, v);
      }
    }
    return ctx.digest();
  };

  it('gives identical frames for the same seed', () => {
    expect(run(42)).toBe(run(42));
  });

  it('gives different frames for a different seed', () => {
    expect(run(42)).not.toBe(run(43));
  });
});

describe('fixed 60 Hz step', () => {
  it('runs at the same speed on 60 Hz and 120 Hz displays', () => {
    const a = new CoreEngine({ seed: 5, demo: true });
    const b = new CoreEngine({ seed: 5, demo: true });
    for (let i = 0; i < 120; i++) a.advance(1 / 60);
    for (let i = 0; i < 240; i++) b.advance(1 / 120);
    expect(b.S.t).toBeCloseTo(a.S.t, 6);
    expect(b.T.ai.E).toBeCloseTo(a.T.ai.E, 9);
  });

  it('caps catch-up after a long pause instead of fast-forwarding', () => {
    const e = new CoreEngine({ seed: 6, demo: true });
    e.advance(2);
    expect(e.S.t).toBeLessThanOrEqual(0.05 + 1e-9);
  });
});
