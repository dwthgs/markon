import { useEffect, useRef } from 'react';

/** Quit asks first. Enter quits, Esc stays. */
export function QuitDialog({ onQuit, onStay }: { onQuit: () => void; onStay: () => void }) {
  const quitRef = useRef<HTMLButtonElement>(null);
  useEffect(() => quitRef.current?.focus(), []);
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60">
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="quit-title"
        className="border border-primary/60 bg-[#04090f] px-8 py-6 font-mono shadow-[0_0_24px_rgba(0,210,235,.25)]"
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.stopPropagation();
            onStay();
          }
        }}
      >
        <h2 id="quit-title" className="font-heading text-sm tracking-[.3em] text-primary">
          QUIT MARKON?
        </h2>
        <p className="mt-3 text-xs text-white/60">Enter to quit · Esc to stay</p>
        <div className="mt-5 flex gap-3">
          <button ref={quitRef} type="button" onClick={onQuit} className="hud-btn hud-btn-on">
            Quit
          </button>
          <button type="button" onClick={onStay} className="hud-btn">
            Stay
          </button>
        </div>
      </div>
    </div>
  );
}
