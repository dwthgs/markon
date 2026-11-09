import type { CoreView } from '@/lib/core';
import { call } from '@/lib/tauri';

/** Top-centre notices: setup problems and background work, never decorative. */
export function Notices({ view }: { view: CoreView }) {
  const s = view.status;
  const items: { id: string; text: string; action?: { label: string; run: () => void } }[] = [];
  if (s && !s.has_key && s.route === 'anthropic')
    items.push({ id: 'key', text: 'No Anthropic key yet. Add one in Settings (Cmd+,) to start talking.' });
  if (s?.model_note) items.push({ id: 'models', text: s.model_note });
  if (view.micProblem === 'denied')
    items.push({
      id: 'mic',
      text: 'Markon cannot use the microphone.',
      action: { label: 'Open microphone settings', run: () => void call('open_mic_settings') },
    });
  if (view.micProblem === 'missing')
    items.push({ id: 'nomic', text: 'No microphone found. You can type instead.' });
  if (view.error) items.push({ id: 'err', text: view.error });
  return (
    <div
      role="status"
      aria-live="polite"
      className="absolute top-4 left-1/2 flex -translate-x-1/2 flex-col items-center gap-2"
    >
      {items.map((n) => (
        <div
          key={n.id}
          className="flex items-center gap-3 border border-primary/30 bg-[#04090f]/90 px-3 py-1.5 font-mono text-[11px] tracking-wider text-white/75"
        >
          <span>{n.text}</span>
          {n.action ? (
            <button type="button" className="hud-btn py-0.5" onClick={n.action.run}>
              {n.action.label}
            </button>
          ) : null}
        </div>
      ))}
    </div>
  );
}
