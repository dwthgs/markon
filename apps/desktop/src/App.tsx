import { type FeatureFeed, VoiceCore } from '@markon/voice-core';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useRef, useState } from 'react';
import { MicMeter } from './hud/MicMeter';
import { Notices } from './hud/Notices';
import { QuitDialog } from './hud/QuitDialog';
import { TextInput } from './hud/TextInput';
import { useCore } from './lib/core';
import { call, inTauri } from './lib/tauri';
import { useViewport } from './lib/useViewport';

const LABEL = {
  idle: 'STANDBY',
  listening: 'LISTENING',
  thinking: 'THINKING',
  speaking: 'SPEAKING',
} as const;

export function App() {
  const { width, height } = useViewport();
  const feed = useRef<FeatureFeed>(null);
  const input = useRef<HTMLInputElement>(null);
  const view = useCore(feed);
  const [quitting, setQuitting] = useState(false);
  // HUD centre is 800 px on a large screen; smaller screens shrink it. The core never moves.
  const size = Math.round(Math.min(800, Math.min(width, height) * 0.78));

  useEffect(() => {
    if (!inTauri()) return;
    const onCommand = (cmd: string) => {
      if (cmd === 'quit') setQuitting(true);
    };
    const subs = [
      listen<string>('markon://menu', (e) => onCommand(e.payload)),
      listen<{ command: string }>('markon://command', (e) => onCommand(e.payload.command)),
    ];
    return () => {
      for (const s of subs) void s.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (view.micProblem === 'missing') input.current?.focus();
  }, [view.micProblem]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || quitting || e.defaultPrevented) return;
      // Canvas cards are cleared first; with nothing on screen Esc hides to the tray.
      void call('hide_window');
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [quitting]);

  return (
    <main className="fixed inset-0 overflow-hidden bg-[radial-gradient(70%_60%_at_50%_45%,#071624_0%,#03060c_70%)]">
      <div className="absolute inset-0 grid place-items-center">
        <VoiceCore variant="prime" state={view.state} size={size} feedRef={feed} demo={!inTauri()} />
      </div>
      <div
        data-testid="status-line"
        className={`absolute left-1/2 -translate-x-1/2 font-mono text-[11px] tracking-[.5em] ${
          view.state === 'thinking' ? 'text-amber-400/85' : 'text-primary/85'
        }`}
        style={{ top: `calc(50% + ${size / 2 - size * 0.06}px)` }}
      >
        {LABEL[view.state]}
      </div>
      <Notices view={view} />
      <MicMeter level={view.level} who={view.levelWho} line={view.line} />
      <TextInput ref={input} />
      {quitting ? (
        <QuitDialog onQuit={() => void call('quit_app')} onStay={() => setQuitting(false)} />
      ) : null}
    </main>
  );
}
