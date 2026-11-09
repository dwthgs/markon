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
import { type PermissionAsk, PermissionDialog } from './hud/PermissionDialog';
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
  const { cards, clear, tidy, toFront } = useCards();
  const [quitting, setQuitting] = useState(false);
  const [asks, setAsks] = useState<PermissionAsk[]>([]);
  const [big, setBig] = useState<Set<string>>(new Set());
  const [focusId, setFocusId] = useState<string | null>(null);
  const [history, setHistory] = useState(false);

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

  const escState = useRef({ quitting, asks: asks.length, cards: cards.length, clearAll, history });
  escState.current = { quitting, asks: asks.length, cards: cards.length, clearAll, history };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = escState.current;
      if (e.key !== 'Escape' || s.quitting || s.asks || e.defaultPrevented) return;
      if (s.history) return setHistory(false);
      // Esc clears the canvas when it has cards, otherwise hides to the tray.
      if (s.cards) s.clearAll();
      else void call('hide_window');
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

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
      <Canvas cards={cards} lay={lay} big={big} focusId={focusId} onOverflowPick={toFront} />
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
      {asks[0] ? <PermissionDialog key={asks[0].id} ask={asks[0]} onAnswer={answer} /> : null}
      {quitting ? (
        <QuitDialog onQuit={() => void call('quit_app')} onStay={() => setQuitting(false)} />
      ) : null}
    </main>
  );
}
