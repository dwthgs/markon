import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';
import type { AgentEvent } from '@/lib/core';
import { inTauri } from '@/lib/tauri';
import { type Card, type CardKind, MEDIA } from './types';

const ORDER = {
  agent: 0,
  result: 1,
  page: 2,
  file: 3,
  app: 4,
  note: 5,
  image: 1,
  audio: 1,
  video: 1,
  music: 1,
} as const satisfies Record<CardKind, number>;

/** Markon's pause_media tool, broadcast to the media cards: `{ which, play }` (MediaBody listens). */
export const MEDIA_EVENT = 'markon-media';
export const picked = (title: string, which: string) => {
  const w = which.trim().toLowerCase();
  return w === 'media' || w === 'all' || w === 'latest' || title.toLowerCase().includes(w);
};

/** Which cards Markon's `close_cards` tool means: "all", "media", "latest" or words from a title. */
export function closing(cards: Card[], which: string): Card[] {
  const w = which.trim().toLowerCase();
  if (w === 'all') return [];
  if (w === 'media') return cards.filter((c) => !MEDIA.has(c.kind));
  const hit = w === 'latest' ? [] : cards.filter((c) => c.title.toLowerCase().includes(w));
  if (hit.length) return cards.filter((c) => !hit.includes(c));
  // "Close it": the newest card.
  const newest = cards.reduce<Card | undefined>(
    (a, c) => (a && a.updated_ms > c.updated_ms ? a : c),
    undefined,
  );
  return cards.filter((c) => c !== newest);
}

/** The canvas's cards, kept in arrival order. The core owns them; the UI only shows and arranges them. */
export function useCards() {
  const [cards, setCards] = useState<Card[]>([]);

  useEffect(() => {
    if (!inTauri()) return;
    const subs = [
      listen<Card>('markon://card', ({ payload: c }) => {
        if (c.status === 'command') {
          const which = String(c.body?.which ?? 'latest');
          if (c.body?.command === 'close') setCards((cs) => closing(cs, which));
          else
            window.dispatchEvent(
              new CustomEvent(MEDIA_EVENT, { detail: { which, play: c.body?.command === 'play' } }),
            );
          return;
        }
        setCards((cs) => {
          const i = cs.findIndex((x) => x.id === c.id);
          if (i < 0) return [...cs, c];
          const next = cs.slice();
          next[i] = c;
          return next;
        });
      }),
      listen<Card[]>('markon://cards-load', ({ payload }) =>
        setCards(payload.filter((c) => c.status !== 'command')),
      ),
      listen<AgentEvent>('markon://agent', ({ payload: e }) => {
        if (e.type === 'turn_cancelled') setCards((cs) => cs.filter((c) => c.turn_id !== e.data.turn_id));
      }),
    ];
    return () => {
      for (const s of subs) void s.then((f) => f());
    };
  }, []);

  const clear = useCallback(() => setCards([]), []);
  const remove = useCallback((id: string) => setCards((cs) => cs.filter((c) => c.id !== id)), []);
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
  return { cards, clear, remove, tidy, toFront };
}
