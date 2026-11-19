import { useEffect, useRef, useState } from 'react';
import { call } from '@/lib/tauri';
import { cn } from '@/lib/utils';
import { CARD_H, GAP } from './layout';
import { MediaBody } from './MediaBody';
import type { Card } from './types';

const KIND = {
  agent: 'Agent',
  file: 'File',
  page: 'Page',
  app: 'App',
  result: 'Result',
  note: 'Note',
  image: 'Picture',
  audio: 'Audio',
  video: 'Video',
  music: 'Music',
  source: 'Source',
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

function SourceBody({ card }: { card: Card }) {
  const b = card.body ?? {};
  if (card.status !== 'done') {
    return (
      <>
        <p className="whitespace-pre-wrap">{b.reason ?? ''}</p>
        {b.needs_full_disk_access ? (
          <button type="button" className="hud-btn mt-2" onClick={() => void call('open_full_disk_access')}>
            Open Full Disk Access settings
          </button>
        ) : null}
      </>
    );
  }
  const items: { who: string; where: string; text: string; unread: boolean }[] = b.items ?? [];
  return (
    <>
      <p className="mb-1 text-[10px] tracking-widest text-white/60">{b.count ?? 0} NEW</p>
      <ul className="space-y-1">
        {items.map((i, n) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: a source card's items never reorder
          <li key={n} className={i.unread ? 'text-white/85' : ''}>
            {i.who ? <span className="text-primary">{i.who}: </span> : null}
            {i.text}
          </li>
        ))}
      </ul>
    </>
  );
}

function Body({ card }: { card: Card }) {
  const b = card.body ?? {};
  switch (card.kind) {
    case 'source':
      return <SourceBody card={card} />;
    case 'agent':
      return <AgentBody card={card} />;
    case 'image':
    case 'audio':
    case 'video':
    case 'music':
      return <MediaBody card={card} />;
    case 'result':
    case 'note':
      return (
        <>
          <p className="whitespace-pre-wrap">{b.text ?? b.preview ?? ''}</p>
          {Array.isArray(b.sources) && b.sources.length ? (
            <ol className="mt-2 list-decimal pl-4 text-[10px] text-white/55" aria-label="Sources">
              {b.sources.map((s: string) => (
                <li key={s} className="break-all">
                  {s}
                </li>
              ))}
            </ol>
          ) : null}
        </>
      );
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
          {b.needs_permission ? (
            <button
              type="button"
              className="hud-btn mt-2"
              onClick={() => void call('open_permission_settings', { permission: b.needs_permission })}
            >
              Open {b.needs_permission === 'screen_recording' ? 'Screen Recording' : 'Accessibility'} settings
            </button>
          ) : null}
        </>
      );
  }
}

export function CardView({
  card,
  big,
  focusRef,
  onClose,
}: {
  card: Card;
  big: boolean;
  focusRef?: (el: HTMLElement | null) => void;
  onClose?: () => void;
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
  // The music card is the player itself: no HUD header, the cover runs edge to edge.
  const bare = card.kind === 'music' && card.status === 'done';
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
        'card-enter relative flex flex-col overflow-hidden border bg-[#040a10]/95 font-mono text-[11px] text-white/70 outline-none transition-[border-color,box-shadow] duration-300 focus-visible:ring-2 focus-visible:ring-primary',
        card.kind === 'agent'
          ? 'border-primary/90 shadow-[0_0_22px_rgba(0,210,235,.35)]'
          : 'border-primary/50 shadow-[0_0_12px_rgba(0,210,235,.18)]',
        bare ? 'p-0' : 'px-3 py-2',
        card.status === 'blocked' && 'border-amber-400/70',
        card.status === 'error' && 'border-red-400/60',
        updated && 'border-white shadow-[0_0_22px_rgba(255,255,255,.35)]',
      )}
    >
      <header
        className={cn(
          'mb-1.5 flex items-center justify-between gap-2 text-[10px] tracking-[.14em] text-white/55',
          bare && 'absolute top-2 right-3 z-10 mb-0',
        )}
      >
        {bare ? null : <span>{`${KIND[card.kind].toUpperCase()} // ${card.status.toUpperCase()}`}</span>}
        <button
          type="button"
          aria-label={`Close ${card.title}`}
          onClick={onClose}
          className="-my-1 -mr-1 px-1.5 py-0.5 text-[12px] text-white/40 hover:text-white focus-visible:text-white"
        >
          ✕
        </button>
      </header>
      <h3
        className={cn(
          'mb-1.5 truncate font-heading text-[12px] tracking-wide text-white normal-case',
          bare && 'sr-only',
        )}
      >
        {card.title}
      </h3>
      <div
        data-testid="card-body"
        className={cn('min-h-0 flex-1 select-text', bare ? 'overflow-hidden' : 'overflow-y-auto')}
      >
        <Body card={card} />
      </div>
    </article>
  );
}
