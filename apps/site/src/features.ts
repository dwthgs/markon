import { clamp, type Features, NB } from '@markon/voice-core/engine';
import { BAND_HZ } from '@markon/voice-core/speech';

/**
 * Live features from a Web Audio analyser, the same mapping the desktop app uses
 * (crates/audio/src/features.rs): loudness, autocorrelation
 * pitch between 70 and 350 Hz, and 48 log-spaced bands from analyser bytes (-100..-30 dB).
 */
export function featuresFrom(time: Float32Array, spectrum: Uint8Array, sampleRate: number): Features {
  let sum = 0;
  for (const x of time) sum += x * x;
  const rms = clamp((Math.sqrt(sum / time.length) - 0.008) * 7, 0, 1);

  let pitch = 0;
  if (rms > 0.05) {
    const n = Math.min(1024, time.length - Math.floor(sampleRate / 70));
    let best = 0;
    let bestLag = 0;
    for (let lag = Math.floor(sampleRate / 350); lag < Math.floor(sampleRate / 70); lag++) {
      let s = 0;
      for (let i = 0; i < n; i++) s += (time[i] ?? 0) * (time[i + lag] ?? 0);
      if (s > best) {
        best = s;
        bestLag = lag;
      }
    }
    if (bestLag) pitch = sampleRate / bestLag;
  }

  const bin = sampleRate / (spectrum.length * 2);
  const bands = new Float32Array(NB);
  for (let k = 0; k < NB; k++) {
    const f = BAND_HZ[k] ?? 0;
    const lo = Math.max(1, Math.floor((f * 0.94) / bin));
    const hi = Math.max(lo + 1, Math.ceil((f * 1.06) / bin));
    let m = 0;
    for (let j = lo; j < hi && j < spectrum.length; j++) m = Math.max(m, spectrum[j] ?? 0);
    bands[k] = clamp((m / 255 - 0.25) * 1.6, 0, 1);
  }
  return { rms, pitch, bands };
}
