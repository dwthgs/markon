import type { Line } from '@/lib/core';

/** Bottom-left: who is audible, their level, and the live transcript. */
export function MicMeter({ level, who, line }: { level: number; who: 'user' | 'markon'; line: Line | null }) {
  const db = level > 1 ? (-60 + (level / 100) * 52).toFixed(1) : '-60.0';
  return (
    <div className="absolute bottom-4 left-4 flex max-w-[min(70ch,45vw)] items-center gap-2.5 font-mono text-[11px] text-white/60">
      <span className="border border-white/30 px-1.5 py-px text-[10px]">
        {who === 'markon' ? 'OUT' : 'MIC'}
      </span>
      <meter
        className="sr-only"
        aria-label="Audio level"
        min={0}
        max={100}
        value={level}
        data-testid="meter"
      />
      <div className="relative h-[5px] w-28 shrink-0 bg-white/10" aria-hidden="true">
        <i
          className="absolute inset-y-0 left-0 bg-primary shadow-[0_0_8px_var(--primary)]"
          style={{ width: `${level}%` }}
        />
      </div>
      <span className="shrink-0">LVL {db} dB</span>
      <span className="truncate text-white/85 select-text" data-testid="transcript">
        {line ? `${line.who}: ${line.text}` : ''}
      </span>
    </div>
  );
}
