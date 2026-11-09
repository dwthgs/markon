import { type FeatureFeed, VoiceCore } from '@markon/voice-core';
import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Canvas } from './canvas/Canvas';
import { layout } from './canvas/layout';
import { useCards } from './canvas/useCards';
import { Controls } from './hud/Controls';
import { HistoryPanel } from './hud/HistoryPanel';
import { MicMeter } from './hud/MicMeter';
import { Notices } from './hud/Notices';
import { Onboarding } from './hud/Onboarding';
import { type PermissionAsk, PermissionDialog } from './hud/PermissionDialog';
import { QuitDialog } from './hud/QuitDialog';
import { SettingsPanel } from './hud/SettingsPanel';
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
  const { cards, clear, tidy, toFront } = useCards();
  const [quitting, setQuitting] = useState(false);
  const [asks, setAsks] = useState<PermissionAsk[]>([]);
  const [big, setBig] = useState<Set<string>>(new Set());
  const [focusId, setFocusId] = useState<string | null>(null);
  const [history, setHistory] = useState(false);
  const [settings, setSettings] = useState(false);
  const [onboardingDone, setOnboardingDone] = useState(false);

  // HUD centre is 800 px on a large screen. The core never moves; with cards it shrinks a little.
  const coreMax = Math.round(Math.min(800, Math.min(width, height) * 0.78));
  const lay = useMemo(() => layout(cards, width, height, big, coreMax), [cards, width, height, big, coreMax]);
  const size = lay.core;

  const clearAll = useCallback(() => {
    clear();
    setBig(new Set());
    setFocusId(null);
  }, [clear]);

  const focusOn = useCallback(
    (words: string) => {
      const w = words.toLowerCase();
      const hit = [...cards].reverse().find((c) => c.title.toLowerCase().includes(w));
      if (!hit) return;
      toFront(hit.id);
      setBig(new Set([hit.id]));
      setFocusId(hit.id);
    },
    [cards, toFront],
  );

  const command = useCallback(
    (cmd: string, arg = '') => {
      if (cmd === 'quit') setQuitting(true);
      else if (cmd === 'settings') setSettings(true);
      else if (cmd === 'clear') clearAll();
      else if (cmd === 'tidy') tidy();
      else if (cmd === 'focus') focusOn(arg);
      else if (cmd === 'bigger') {
        const id = focusId ?? cards.at(-1)?.id;
        if (id) setBig((b) => new Set(b).add(id));
      } else if (cmd === 'bring_back') void call('bring_back', { topic: arg });
    },
    [clearAll, tidy, focusOn, focusId, cards],
  );
  const commandRef = useRef(command);
  commandRef.current = command;

  useEffect(() => {
    if (!inTauri()) return;
    const subs = [
      listen<string>('markon://menu', (e) => commandRef.current(e.payload)),
      listen<{ command: string; arg: string }>('markon://command', (e) =>
        commandRef.current(e.payload.command, e.payload.arg),
      ),
      listen<PermissionAsk>('markon://permission', (e) => setAsks((a) => [...a, e.payload])),
    ];
    return () => {
      for (const s of subs) void s.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (view.micProblem === 'missing') input.current?.focus();
  }, [view.micProblem]);

  const answer = (allow: boolean) => {
    const ask = asks[0];
    if (!ask) return;
    void call('answer_permission', { id: ask.id, decision: allow ? 'allow' : 'deny' });
    setAsks((a) => a.slice(1));
  };

  const escState = useRef({
    quitting,
    asks: asks.length,
    cards: cards.length,
    clearAll,
    history,
    settings,
    deny: () => answer(false),
  });
  escState.current = {
    quitting,
    asks: asks.length,
    cards: cards.length,
    clearAll,
    history,
    settings,
    deny: () => answer(false),
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = escState.current;
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      // Esc closes the top layer first, even before focus has moved into it.
      if (s.asks) return s.deny();
      if (s.quitting) return setQuitting(false);
      if (s.history) return setHistory(false);
      if (s.settings) return setSettings(false);
      // Esc clears the canvas when it has cards, otherwise hides to the tray.
      if (s.cards) s.clearAll();
      else void call('hide_window');
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <main
      data-reduced-motion={view.status?.reduced_motion ?? false}
      className="fixed inset-0 overflow-hidden bg-[radial-gradient(70%_60%_at_50%_45%,#071624_0%,#03060c_70%)]"
    >
      <div className="absolute inset-0 grid place-items-center">
        <VoiceCore
          variant="prime"
          state={view.state}
          size={size}
          feedRef={feed}
          demo={!inTauri()}
          {...(view.status?.reduced_motion ? { reducedMotion: true } : {})}
        />
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
      <Canvas cards={cards} lay={lay} focusId={focusId} onOverflowPick={toFront} />
      {/* drag here to move the window; double-click fills the screen again.
          Sits under the controls and notices so their buttons stay clickable. */}
      <div
        data-tauri-drag-region
        data-testid="drag-strip"
        title="Drag to move"
        className="group absolute inset-x-0 top-0 grid h-7 place-items-center"
      >
        <div className="pointer-events-none h-1 w-16 rounded-full bg-primary/20 transition-colors group-hover:bg-primary/60" />
      </div>
      <Notices view={view} />
      <Controls
        onHistory={() => setHistory((h) => !h)}
        onTidy={tidy}
        onClear={clearAll}
        onCamera={() => void call('capture_window')}
      />
      <MicMeter level={view.level} who={view.levelWho} line={view.line} />
      <TextInput ref={input} />
      {history ? <HistoryPanel onClose={() => setHistory(false)} /> : null}
      {settings ? <SettingsPanel onClose={() => setSettings(false)} /> : null}
      {view.status && !view.status.onboarded && !onboardingDone ? (
        <Onboarding onDone={() => setOnboardingDone(true)} />
      ) : null}
      {asks[0] ? <PermissionDialog key={asks[0].id} ask={asks[0]} onAnswer={answer} /> : null}
      {quitting ? (
        <QuitDialog onQuit={() => void call('quit_app')} onStay={() => setQuitting(false)} />
      ) : null}
    </main>
  );
}
