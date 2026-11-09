import { useEffect, useRef } from 'react';

export interface PermissionAsk {
  id: number;
  tier: 'read' | 'write' | 'execute';
  tool: string;
  detail: string;
}

const TITLE = {
  read: 'Allow Markon to read this?',
  write: 'Allow Markon to write in this folder?',
  execute: 'Allow Markon to run this?',
} as const;

/** Shows exactly what will happen. No default button: the user must choose. Esc denies. */
export function PermissionDialog({
  ask,
  onAnswer,
}: {
  ask: PermissionAsk;
  onAnswer: (allow: boolean) => void;
}) {
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
            onAnswer(false);
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
        <div className="mt-5 flex gap-3">
          <button type="button" className="hud-btn hud-btn-on" onClick={() => onAnswer(true)}>
            Allow
          </button>
          <button ref={denyRef} type="button" className="hud-btn" onClick={() => onAnswer(false)}>
            Deny
          </button>
        </div>
      </div>
    </div>
  );
}
