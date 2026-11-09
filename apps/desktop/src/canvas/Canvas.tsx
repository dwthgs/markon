import { type CSSProperties, useEffect, useRef } from 'react';
import { CardView } from './CardView';
import { GAP, type Layout } from './layout';
import type { Card } from './types';

interface Props {
  cards: Card[];
  lay: Layout;
  focusId: string | null;
  onOverflowPick: (id: string) => void;
}

export function Canvas({ cards, lay, focusId, onOverflowPick }: Props) {
  const byId = new Map(cards.map((c) => [c.id, c]));
  const refs = useRef(new Map<string, HTMLElement>());
  useEffect(() => {
    if (focusId) refs.current.get(focusId)?.focus();
  }, [focusId]);

  const zone = (ids: string[], side: 'left' | 'right') => {
    const style: CSSProperties = { width: lay.zoneW, gap: GAP, [side]: 16 };
    return (
      <div className="absolute top-14 bottom-14 flex flex-col" style={style}>
        {ids.map((id) => {
          const c = byId.get(id);
          return c ? (
            <CardView
              key={id}
              card={c}
              big={lay.big.has(id)}
              focusRef={(el) => {
                if (el) refs.current.set(id, el);
                else refs.current.delete(id);
              }}
            />
          ) : null;
        })}
      </div>
    );
  };

  return (
    <>
      <section data-testid="canvas-zone" aria-label="Workspace" className="contents">
        {lay.columns === 2 ? zone(lay.left, 'left') : null}
        {zone(lay.right, 'right')}
      </section>
      {lay.overflow.length ? (
        <div className="absolute right-4 bottom-12 flex max-w-[40vw] items-center gap-2 overflow-x-auto font-mono text-[10px]">
          <span
            data-testid="overflow-badge"
            className="shrink-0 border border-primary/60 px-1.5 py-px text-primary"
          >
            +{lay.overflow.length}
          </span>
          {lay.overflow.map((id) => (
            <button
              key={id}
              type="button"
              className="hud-btn shrink-0 py-0.5"
              onClick={() => onOverflowPick(id)}
            >
              {byId.get(id)?.title}
            </button>
          ))}
        </div>
      ) : null}
    </>
  );
}
