// Test harness: drives the port with the same clock the reference page gets in e2e tests.
import { drawVariant, type Variant } from '../src/draw';
import { CoreEngine, type CoreState } from '../src/engine';

declare global {
  interface Window {
    harness: {
      setup(seed: number, variant: Variant, W: number, demo: boolean): void;
      setState(s: CoreState): void;
      tick(n: number): void;
      timeFrames(n: number, batch?: number): number[];
      pixels(): string;
    };
  }
}

const cv = document.getElementById('cv') as HTMLCanvasElement;
const c = cv.getContext('2d') as CanvasRenderingContext2D;
let engine = new CoreEngine();
let variant: Variant = 'prime';
let W = 400;
let now = 0;
let last = 0;

window.harness = {
  setup(seed, v, w, demo) {
    engine = new CoreEngine({ seed, demo });
    variant = v;
    W = w;
    cv.width = w;
    cv.height = w;
    now = 0;
    last = 0;
  },
  setState: (s) => engine.setState(s),
  // Same dt sequence as the reference page under the fake clock.
  tick(n) {
    for (let i = 0; i < n; i++) {
      now += 1000 / 60;
      engine.step(Math.min(0.05, (now - last) / 1000));
      last = now;
      drawVariant(engine, c, W, variant);
    }
  },
  // Mean ms per frame over batches of `batch` frames; a pixel read ends each batch so rasterising is included.
  timeFrames(n, batch = 20) {
    const out: number[] = [];
    for (let i = 0; i < n; i += batch) {
      const t0 = performance.now();
      for (let j = 0; j < batch; j++) {
        engine.advance(1 / 60);
        drawVariant(engine, c, W, variant);
      }
      c.getImageData(0, 0, 1, 1);
      out.push((performance.now() - t0) / batch);
    }
    return out;
  },
  pixels: () => cv.toDataURL('image/png'),
};
