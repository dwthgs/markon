import type { Card } from './types';

export const CARD_H = 168;
export const GAP = 12;
const MARGIN_Y = 56;
const MIN_CORE = 220;

export interface Layout {
  core: number;
  zoneW: number;
  columns: 1 | 2;
  /** Card ids per zone, top to bottom. With one column only `right` is used. */
  left: string[];
  right: string[];
  overflow: string[];
}

/**
 * Cards sit in side zones of a 12-column grid (columns 1-3 and 10-12) around the centred core.
 * The core shrinks a little to make room but never moves. Cards of one turn stay together.
 */
export function layout(
  cards: Card[],
  width: number,
  height: number,
  big: Set<string>,
  coreMax: number,
): Layout {
  const narrow = width < 1024;
  const zoneW = narrow ? Math.min(300, width * 0.32) : Math.min(380, (width * 3) / 12);
  const room = narrow ? width - 2 * (zoneW + GAP * 2) : width - 2 * (zoneW + GAP * 2);
  const shrink = cards.length ? 0.85 : 1;
  const core = Math.max(MIN_CORE, Math.round(Math.min(coreMax * shrink, cards.length ? room : coreMax)));
  const slots = Math.max(1, Math.floor((height - 2 * MARGIN_Y + GAP) / (CARD_H + GAP)));
  const zones: { ids: string[]; used: number }[] = narrow
    ? [{ ids: [], used: 0 }]
    : [
        { ids: [], used: 0 },
        { ids: [], used: 0 },
      ];
  const overflow: string[] = [];
  const turns = new Map<number, Card[]>();
  for (const c of cards) turns.set(c.turn_id, [...(turns.get(c.turn_id) ?? []), c]);
  for (const group of turns.values()) {
    for (const c of group) {
      const need = big.has(c.id) ? 2 : 1;
      // Keep a turn's cards on the side that has most room when the turn starts.
      const order = [...zones].sort((a, b) => a.used - b.used);
      const z = order.find((z) => z.used + need <= slots);
      if (z) {
        z.ids.push(c.id);
        z.used += need;
      } else overflow.push(c.id);
    }
  }
  return {
    core,
    zoneW,
    columns: narrow ? 1 : 2,
    left: narrow ? [] : (zones[0]?.ids ?? []),
    right: narrow ? (zones[0]?.ids ?? []) : (zones[1]?.ids ?? []),
    overflow,
  };
}
