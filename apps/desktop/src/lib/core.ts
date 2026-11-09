import type { CoreState, FeatureFeed } from '@markon/voice-core';
import { listen } from '@tauri-apps/api/event';
import { type RefObject, useCallback, useEffect, useState } from 'react';
import { call, inTauri } from './tauri';

/* Shapes of the events the Rust core sends (apps/desktop/src-tauri/src/core.rs). */
export interface Status {
  state: CoreState;
  models_ready: boolean;
  model_note: string;
  has_key: boolean;
  voice: 'markon' | 'markan' | null;
  route: 'anthropic' | 'openrouter' | 'relay';
  listening: boolean;
  hotkey: string;
  onboarded: boolean;
  version: string;
}

export type AgentEvent =
  | { type: 'state_changed'; data: CoreState }
  | { type: 'text_delta'; data: { turn_id: number; text: string } }
  | { type: 'tool_call_started'; data: { turn_id: number; id: string; name: string; input: unknown } }
  | {
      type: 'tool_call_finished';
      data: { turn_id: number; id: string; name: string; ok: boolean; output: string };
    }
  | { type: 'turn_finished'; data: { turn_id: number; user: string; text: string; partial: boolean } }
  | { type: 'turn_cancelled'; data: { turn_id: number } }
  | { type: 'error'; data: string };

export type AudioEvent =
  | { type: 'partial_transcript'; text: string }
  | { type: 'final_transcript'; text: string; ms: number }
  | { type: 'features'; who: 'user' | 'markon'; rms: number; pitch: number; bands: number[] }
  | { type: 'error'; message: string }
  | {
      type:
        | 'speech_started'
        | 'speech_ended'
        | 'playback_started'
        | 'playback_ended'
        | 'no_microphone'
        | 'mic_denied'
        | 'models_loading'
        | 'models_ready';
    }
  | { type: 'model_mismatch'; id: string };

export interface Line {
  who: 'YOU' | 'MARKON';
  text: string;
}

export interface CoreView {
  status: Status | null;
  state: CoreState;
  line: Line | null;
  level: number;
  levelWho: 'user' | 'markon';
  micProblem: 'denied' | 'missing' | null;
  error: string | null;
}

/** Subscribe to the Rust core. Live audio features go straight to the voice core through `feed`. */
export function useCore(feed: RefObject<FeatureFeed | null>) {
  const [view, setView] = useState<CoreView>({
    status: null,
    state: 'idle',
    line: null,
    level: 0,
    levelWho: 'user',
    micProblem: null,
    error: null,
  });

  const refresh = useCallback(async () => {
    const status = await call<Status>('status');
    if (status) setView((v) => ({ ...v, status, state: status.state }));
  }, []);

  useEffect(() => {
    if (!inTauri()) return;
    void refresh();
    let level = 0;
    const unlisten = [
      listen<Status>('markon://status', (e) => setView((v) => ({ ...v, status: e.payload }))),
      listen<AgentEvent>('markon://agent', ({ payload: e }) =>
        setView((v) => {
          switch (e.type) {
            case 'state_changed':
              return { ...v, state: e.data, error: e.data === 'listening' ? null : v.error };
            case 'text_delta': {
              const prev = v.line?.who === 'MARKON' ? v.line.text : '';
              return { ...v, line: { who: 'MARKON', text: prev + e.data.text } };
            }
            case 'turn_finished':
              return { ...v, line: { who: 'MARKON', text: e.data.text } };
            case 'error':
              return { ...v, error: e.data };
            default:
              return v;
          }
        }),
      ),
      listen<AudioEvent>('markon://audio', ({ payload: e }) =>
        setView((v) => {
          switch (e.type) {
            case 'partial_transcript':
            case 'final_transcript':
              return { ...v, line: { who: 'YOU', text: e.text } };
            case 'mic_denied':
              return { ...v, micProblem: 'denied' };
            case 'no_microphone':
              return { ...v, micProblem: 'missing' };
            case 'model_mismatch':
              return {
                ...v,
                error: `The ${e.id} file does not match its pinned checksum, so Markon will not speak.`,
              };
            case 'error':
              return { ...v, error: e.message };
            default:
              return v;
          }
        }),
      ),
      listen<Extract<AudioEvent, { type: 'features' }>>('markon://features', ({ payload: f }) => {
        feed.current?.push(f);
        // The meter only needs a few updates a second of React state.
        const pct = Math.round(f.rms * 100);
        if (Math.abs(pct - level) >= 2 || pct === 0) {
          level = pct;
          setView((v) => ({ ...v, level: pct, levelWho: f.who }));
        }
      }),
    ];
    return () => {
      for (const u of unlisten) void u.then((f) => f());
    };
  }, [feed, refresh]);

  return view;
}
