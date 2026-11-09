import { invoke } from '@tauri-apps/api/core';

/** True inside the Tauri window; false in a plain browser (Playwright, `vite`). */
export const inTauri = () => '__TAURI_INTERNALS__' in window;

export function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
  return inTauri() ? invoke<T>(cmd, args) : Promise.resolve(undefined);
}
