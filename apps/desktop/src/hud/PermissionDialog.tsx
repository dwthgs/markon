import { useEffect, useRef } from 'react';

export interface PermissionAsk {
  id: number;
  tier: 'read' | 'write' | 'execute' | 'send';
  tool: string;
  detail: string;
  /** For a send: who it goes to; "always yes" is offered only when `always_ok`. */
  pattern?: string | null;
  always_ok?: boolean;
}

export type Answer = 'allow' | 'deny' | 'always';

const TITLE = {
  read: 'Allow Markon to read this?',
  write: 'Allow Markon to write in this folder?',
  execute: 'Allow Markon to run this?',
  send: 'Send this?',
} as const;

/** Shows exactly what will happen. No default button: the user must choose. Esc denies. */
export function PermissionDialog({ ask, onAnswer }: { ask: PermissionAsk; onAnswer: (a: Answer) => void }) {
  const denyRef = useRef<HTMLButtonElement>(null);
  useEffect(() => denyRef.current?.focus(), []);
  return (
    <div className="fixed inset-0 z-40 grid place-items-center bg-black/55">
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="perm-title"
        className="w-[min(560px,90vw)] border border-amber-400/70 bg-[#04090f] px-6 py-5 font-mono shadow-[0_0_24px_rgba(255,176,40,.25)]"
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.stopPropagation();
            onAnswer('deny');
          }
        }}
      >
        <h2 id="perm-title" className="font-heading text-sm tracking-[.2em] text-amber-300">
          {TITLE[ask.tier]}
        </h2>
        <p className="mt-2 text-[11px] text-white/55">{ask.tool.replaceAll('_', ' ')}</p>
        <pre className="mt-3 max-h-48 overflow-auto border border-white/15 bg-black/40 p-3 text-xs whitespace-pre-wrap text-white select-text">
          {ask.detail}
        </pre>
        {ask.tier === 'send' ? (
          <p className="mt-2 text-[11px] text-white/55">Or say "yes, send it", "no", or "always yes".</p>
        ) : null}
        <div className="mt-5 flex gap-3">
          <button type="button" className="hud-btn hud-btn-on" onClick={() => onAnswer('allow')}>
            {ask.tier === 'send' ? 'Send' : 'Allow'}
          </button>
          <button ref={denyRef} type="button" className="hud-btn" onClick={() => onAnswer('deny')}>
            {ask.tier === 'send' ? "Don't send" : 'Deny'}
          </button>
          {ask.tier === 'send' && ask.always_ok && ask.pattern ? (
            <button type="button" className="hud-btn" onClick={() => onAnswer('always')}>
              Always yes for {ask.pattern}
            </button>
          ) : null}
        </div>
      </div>
    </div>
  );
}
