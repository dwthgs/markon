import { call } from './tauri';

/** Dev builds only: report the real window's frame rate to the Rust log every 2 s. */
export function startFpsProbe() {
  let frames = 0;
  let since = performance.now();
  const tick = (now: number) => {
    frames++;
    if (now - since >= 2000) {
      void call('report_fps', { fps: (frames * 1000) / (now - since) });
      frames = 0;
      since = now;
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}
