import { describe, expect, it } from 'vitest';
import { layout } from './layout';
import type { Card } from './types';

const card = (id: string, turn = 1): Card => ({
  id,
  turn_id: turn,
  kind: 'note',
  title: id,
  status: 'done',
  body: {},
  updated_ms: 0,
});

describe('canvas layout', () => {
  it('keeps the core at full size with no cards and shrinks it a little with cards', () => {
    expect(layout([], 1440, 900, new Set(), 700).core).toBe(700);
    const l = layout([card('a')], 1440, 900, new Set(), 700);
    expect(l.core).toBeLessThan(700);
    expect(l.core).toBeGreaterThanOrEqual(700 * 0.75);
  });

  it('keeps a turn together on one side', () => {
    const l = layout([card('a', 1), card('b', 1), card('c', 2)], 1440, 900, new Set(), 700);
    const sideOf = (id: string) => (l.left.includes(id) ? 'L' : 'R');
    expect(sideOf('a')).toBe(sideOf('b'));
    expect(sideOf('c')).not.toBe(sideOf('a'));
  });

  it('overflows what does not fit, and big cards take two slots', () => {
    const many = Array.from({ length: 30 }, (_, i) => card(`c${i}`, i));
    const l = layout(many, 1440, 900, new Set(), 700);
    expect(l.left.length + l.right.length + l.overflow.length).toBe(30);
    expect(l.overflow.length).toBeGreaterThan(0);
    const big = layout([card('x'), card('y')], 1440, 400, new Set(['x']), 700);
    expect(big.overflow).toEqual([]);
    expect(big.big.size).toBe(0);
    expect(layout([card('x')], 1440, 900, new Set(['x']), 700).big.has('x')).toBe(true);
  });

  it('uses one column below 1024 px', () => {
    const l = layout([card('a'), card('b', 2)], 900, 800, new Set(), 600);
    expect(l.columns).toBe(1);
    expect(l.left).toEqual([]);
    expect(l.right).toEqual(['a', 'b']);
  });
});
