import { useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/utils';
import { CARD_H, GAP } from './layout';
import type { Card } from './types';

const KIND = {
  agent: 'Agent',
  file: 'File',
  page: 'Page',
  app: 'App',
  result: 'Result',
  note: 'Note',
} as const;

function elapsed(ms: number) {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

function AgentBody({ card }: { card: Card }) {
  const b = card.body ?? {};
  const steps: { name: string; status: string }[] = b.steps ?? [];
  const done = steps.filter((s) => s.status !== 'running').length;
  const [, tick] = useState(0);
  useEffect(() => {
    if (card.status !== 'running') return;
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [card.status]);
  return (
    <>
      <div className="mb-1.5 flex gap-0.5" aria-hidden="true">
        {steps.map((s, i) => (
          <i
            // biome-ignore lint/suspicious/noArrayIndexKey: steps only ever append
            key={i}
            className={cn(
              'h-1.5 flex-1',
              s.status === 'running' ? 'bg-white/15' : 'bg-primary shadow-[0_0_6px_var(--primary)]',
            )}
          />
        ))}
      </div>
      <div className="flex justify-between text-[10px] tracking-widest text-white/70">
        <span>
          {done}/{steps.length} STEPS · {b.sources ?? 0} SOURCES
        </span>
        <span>{b.started_ms ? elapsed(b.started_ms) : ''}</span>
      </div>
      <ul className="mt-1.5 space-y-0.5 text-[10px] text-white/60">
        {steps.slice(-4).map((s, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: steps only ever append
          <li key={i}>
            {s.status === 'running' ? '▸' : s.status === 'done' ? '✓' : '✕'} {s.name.replaceAll('_', ' ')}
          </li>
        ))}
      </ul>
    </>
  );
}

function Body({ card }: { card: Card }) {
  const b = card.body ?? {};
  switch (card.kind) {
    case 'agent':
      return <AgentBody card={card} />;
    case 'result':
    case 'note':
      return <p className="whitespace-pre-wrap">{b.text ?? b.preview ?? ''}</p>;
    case 'page':
      return (
        <>
          {b.title ? <p className="mb-1 text-white/85">{b.title}</p> : null}
          <p className="whitespace-pre-wrap">{b.preview ?? b.reason ?? ''}</p>
        </>
      );
    case 'file':
      return Array.isArray(b.entries) ? (
        <ul>
          {b.entries.map((e: { name: string; dir: boolean }) => (
            <li key={e.name}>
              {e.name}
              {e.dir ? '/' : ''}
            </li>
          ))}
        </ul>
      ) : (
        <pre className="whitespace-pre-wrap">{b.preview ?? b.reason ?? ''}</pre>
      );
    default:
      return (
        <>
          {b.command ? <p className="mb-1 text-white/85">$ {b.command}</p> : null}
          <pre className="whitespace-pre-wrap">{b.preview ?? b.action ?? b.reason ?? ''}</pre>
        </>
      );
  }
}

export function CardView({
  card,
  big,
  focusRef,
}: {
  card: Card;
  big: boolean;
  focusRef?: (el: HTMLElement | null) => void;
}) {
  const [updated, setUpdated] = useState(false);
  const prev = useRef(card.updated_ms);
  useEffect(() => {
    if (card.updated_ms === prev.current) return;
    prev.current = card.updated_ms;
    setUpdated(true);
    const t = setTimeout(() => setUpdated(false), 1200);
    return () => clearTimeout(t);
  }, [card.updated_ms]);
  const label = `${KIND[card.kind]}: ${card.title}`;
  return (
    <article
      ref={focusRef}
      aria-label={label}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: every card is keyboard reachable
      tabIndex={0}
      data-updated={updated}
      data-big={big}
      style={{ height: big ? CARD_H * 2 + GAP : CARD_H }}
      className={cn(
        'card-enter flex flex-col border bg-[#040a10]/95 px-3 py-2 font-mono text-[11px] text-white/70 outline-none transition-[border-color,box-shadow] duration-300 focus-visible:ring-2 focus-visible:ring-primary',
        card.kind === 'agent'
          ? 'border-primary/90 shadow-[0_0_22px_rgba(0,210,235,.35)]'
          : 'border-primary/50 shadow-[0_0_12px_rgba(0,210,235,.18)]',
        card.status === 'blocked' && 'border-amber-400/70',
        card.status === 'error' && 'border-red-400/60',
        updated && 'border-white shadow-[0_0_22px_rgba(255,255,255,.35)]',
      )}
    >
      <header className="mb-1.5 flex items-center justify-between gap-2 text-[10px] tracking-[.14em] text-white/55">
        <span>{`${KIND[card.kind].toUpperCase()} // ${card.status.toUpperCase()}`}</span>
        <span aria-hidden="true" className="text-white/30">
          ▢
        </span>
      </header>
      <h3 className="mb-1.5 truncate font-heading text-[12px] tracking-wide text-white normal-case">
        {card.title}
      </h3>
      <div data-testid="card-body" className="min-h-0 flex-1 overflow-y-auto select-text">
        <Body card={card} />
      </div>
    </article>
  );
}
