import { useEffect, useRef, useState } from 'react';
import { call } from '@/lib/tauri';

interface Conversation {
  id: number;
  started_at: number;
  title: string;
  source: 'voice' | 'text' | 'schedule';
}

/** Past conversations; choosing one reloads its cards onto the canvas. */
export function HistoryPanel({ onClose }: { onClose: () => void }) {
  const [items, setItems] = useState<Conversation[] | null>(null);
  const first = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    void call<Conversation[]>('history_list').then((r) => setItems(r ?? []));
  }, []);
  useEffect(() => {
    if (items?.length) first.current?.focus();
  }, [items]);
  return (
    <div
      role="dialog"
      aria-label="History"
      className="absolute top-12 right-4 z-30 max-h-[60vh] w-[min(360px,40vw)] overflow-y-auto border border-primary/50 bg-[#04090f]/95 p-3 font-mono text-[11px]"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <h2 className="mb-2 text-[10px] tracking-[.3em] text-white/55">HISTORY</h2>
      {items === null ? null : items.length === 0 ? (
        <p className="text-white/50">Nothing yet. Conversations appear here after you talk to Markon.</p>
      ) : (
        <ul className="space-y-1">
          {items.map((c, i) => (
            <li key={c.id}>
              <button
                ref={i === 0 ? first : undefined}
                type="button"
                className="hud-btn w-full truncate py-1 text-left normal-case tracking-normal"
                onClick={() => {
                  void call('open_conversation', { id: c.id });
                  onClose();
                }}
              >
                <span className="text-white/45">{new Date(c.started_at).toLocaleString()} · </span>
                {c.title || 'Untitled'}
                {c.source === 'schedule' ? ' · scheduled' : ''}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
