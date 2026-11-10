import type { SettingsView } from '@/lib/settings';
import { call } from '@/lib/tauri';

const ms = (v: number | null) => (v === null ? '–' : v >= 1000 ? `${(v / 1000).toFixed(1)} s` : `${v} ms`);

/** Where each reply's time went, and improvements proposed from it (nothing changes without a yes). */
export function TimeSection({
  view,
  reload,
  attempt,
}: {
  view: SettingsView;
  reload: () => Promise<void>;
  attempt: (f: () => Promise<unknown>) => Promise<void>;
}) {
  return (
    <div className="space-y-2">
      {view.traces.length ? (
        <table className="w-full text-left text-[10px]">
          <thead className="text-white/45">
            <tr>
              <th>Asked</th>
              <th>Heard</th>
              <th>First word</th>
              <th>First sound</th>
              <th>Total</th>
            </tr>
          </thead>
          <tbody>
            {view.traces.map((t) => (
              <tr key={t.id} className={t.ok ? '' : 'text-amber-300'}>
                <td
                  className="max-w-[120px] truncate"
                  title={t.tools.map((x) => `${x.name} ${ms(x.ms)}`).join(', ')}
                >
                  {t.user_text}
                </td>
                <td>{ms(t.stt_ms)}</td>
                <td>{ms(t.first_text_ms)}</td>
                <td>{ms(t.first_audio_ms)}</td>
                <td>{ms(t.total_ms)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p>No replies timed yet.</p>
      )}
      <button
        type="button"
        className="hud-btn py-0.5"
        onClick={() =>
          void attempt(async () => {
            await call('suggest_improvements');
            await reload();
          })
        }
      >
        Suggest improvements
      </button>
      {view.proposals.map((p) => (
        <div key={p.path} className="border border-white/10 p-2">
          <p className="truncate text-white/85">{p.path}</p>
          <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap text-[10px]">{p.diff}</pre>
          <button
            type="button"
            className="hud-btn mt-1 py-0"
            onClick={() =>
              void attempt(async () => {
                await call('approve_proposal', { path: p.path });
                await reload();
              })
            }
          >
            Approve change
          </button>
          <button
            type="button"
            className="hud-btn mt-1 ml-2 py-0"
            onClick={() =>
              void attempt(async () => {
                await call('reject_proposal', { path: p.path });
                await reload();
              })
            }
          >
            Reject change
          </button>
        </div>
      ))}
    </div>
  );
}
