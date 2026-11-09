import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';
import type { AgentEvent } from '@/lib/core';
import { inTauri } from '@/lib/tauri';
import type { Card } from './types';

const ORDER = { agent: 0, result: 1, page: 2, file: 3, app: 4, note: 5 } as const;

/** The canvas's cards, kept in arrival order. The core owns them; the UI only shows and arranges them. */
export function useCards() {
  const [cards, setCards] = useState<Card[]>([]);

  useEffect(() => {
    if (!inTauri()) return;
    const subs = [
      listen<Card>('markon://card', ({ payload: c }) =>
        setCards((cs) => {
          const i = cs.findIndex((x) => x.id === c.id);
          if (i < 0) return [...cs, c];
          const next = cs.slice();
          next[i] = c;
          return next;
        }),
      ),
      listen<Card[]>('markon://cards-load', ({ payload }) => setCards(payload)),
      listen<AgentEvent>('markon://agent', ({ payload: e }) => {
        if (e.type === 'turn_cancelled') setCards((cs) => cs.filter((c) => c.turn_id !== e.data.turn_id));
      }),
    ];
    return () => {
      for (const s of subs) void s.then((f) => f());
    };
  }, []);

  const clear = useCallback(() => setCards([]), []);
  /** Group by turn (newest first), agent and result cards on top, finished agents last. */
  const tidy = useCallback(
    () =>
      setCards((cs) =>
        cs
          .slice()
          .sort(
            (a, b) =>
              b.turn_id - a.turn_id ||
              ORDER[a.kind] - ORDER[b.kind] ||
              Number(a.status === 'done') - Number(b.status === 'done'),
          ),
      ),
    [],
  );
  const toFront = useCallback(
    (id: string) => setCards((cs) => [...cs.filter((c) => c.id === id), ...cs.filter((c) => c.id !== id)]),
    [],
  );
  return { cards, clear, tidy, toFront };
}
