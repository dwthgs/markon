import { describe, expect, it } from 'vitest';
import { featuresFrom } from './features';

const SR = 48000;
const FFT = 2048;

describe('featuresFrom', () => {
  it('reads silence as silence', () => {
    const f = featuresFrom(new Float32Array(FFT), new Uint8Array(FFT / 2), SR);
    expect(f.rms).toBe(0);
    expect(f.pitch).toBe(0);
    expect(Array.from(f.bands).every((b) => b === 0)).toBe(true);
  });

  it('finds the pitch of a 200 Hz tone and lights the band at 1 kHz', () => {
    const time = Float32Array.from({ length: FFT }, (_, i) => 0.3 * Math.sin((2 * Math.PI * 200 * i) / SR));
    const spectrum = new Uint8Array(FFT / 2);
    spectrum[Math.round(1000 / (SR / FFT))] = 255;
    const f = featuresFrom(time, spectrum, SR);
    expect(f.rms).toBeGreaterThan(0.5);
    expect(f.pitch).toBeGreaterThan(195);
    expect(f.pitch).toBeLessThan(205);
    expect(Math.max(...Array.from(f.bands))).toBeGreaterThan(0.9);
  });
});
