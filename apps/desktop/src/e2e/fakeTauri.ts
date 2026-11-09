// Dev-only fake of the Rust side for Playwright. Loaded when the URL has ?e2e.
import { emit } from '@tauri-apps/api/event';
import { mockIPC } from '@tauri-apps/api/mocks';

type Call = { cmd: string; args: unknown };

declare global {
  interface Window {
    __e2e: {
      calls: Call[];
      status: Record<string, unknown>;
      responses: Record<string, unknown>;
      emit: (event: string, payload: unknown) => Promise<void>;
    };
  }
}

export function installFakeTauri() {
  const status = {
    state: 'idle',
    models_ready: true,
    model_note: '',
    has_key: true,
    voice: 'markon',
    route: 'anthropic',
    listening: false,
    hotkey: 'Alt+Space',
    onboarded: true,
    version: '0.1.0',
  };
  const params = new URLSearchParams(location.search);
  for (const [k, v] of params) if (k in status) (status as Record<string, unknown>)[k] = JSON.parse(v);
  window.__e2e = {
    calls: [],
    status,
    responses: {},
    emit: (event, payload) => emit(event, payload),
  };
  mockIPC(
    (cmd, args) => {
      window.__e2e.calls.push({ cmd, args });
      if (cmd === 'status') return window.__e2e.status;
      return window.__e2e.responses[cmd] ?? null;
    },
    { shouldMockEvents: true },
  );
}
