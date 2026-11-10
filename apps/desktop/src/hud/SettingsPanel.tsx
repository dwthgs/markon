import { useEffect, useRef, useState } from 'react';
import { ROUTES, useSettings } from '@/lib/settings';
import { call } from '@/lib/tauri';
import { AutomationSection } from './AutomationSection';
import { KeyField } from './KeyField';
import { SourcesSection } from './SourcesSection';

const nf = new Intl.NumberFormat('en-US');

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="border-t border-white/10 py-3">
      <h3 className="mb-2 text-[10px] tracking-[.3em] text-white/50">{title}</h3>
      {children}
    </section>
  );
}

/** In-HUD settings (Cmd+,). Everything saves immediately; secrets go to the keychain. */
export function SettingsPanel({ onClose }: { onClose: () => void }) {
  const { view, reload, save } = useSettings();
  const [exported, setExported] = useState('');
  const [wiping, setWiping] = useState(false);
  const [hotkey, setHotkey] = useState('');
  const [error, setError] = useState('');
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => ref.current?.focus(), []);
  if (!view) return null;
  const s = view.settings;
  const attempt = async (f: () => Promise<unknown>) => {
    try {
      setError('');
      await f();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div
      ref={ref}
      role="dialog"
      aria-label="Settings"
      tabIndex={-1}
      className="absolute top-12 right-4 bottom-14 z-30 w-[min(440px,46vw)] overflow-y-auto border border-primary/50 bg-[#04090f]/97 px-4 py-3 font-mono text-[11px] text-white/75 outline-none"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="mb-2 flex items-center justify-between">
        <h2 className="font-heading text-sm tracking-[.25em] text-primary">SETTINGS</h2>
        <button type="button" className="hud-btn py-0.5" onClick={onClose}>
          Close <span className="text-white/40">Esc</span>
        </button>
      </div>
      {error ? <p className="mb-2 text-red-300">{error}</p> : null}

      <Section title="VOICE">
        {(['markon', 'markan'] as const).map((v) => (
          <label key={v} className="mr-4">
            <input
              type="radio"
              name="s-voice"
              checked={s.voice === v}
              onChange={() => void attempt(() => save({ voice: v }))}
            />{' '}
            {v === 'markon' ? 'Markon' : 'Markan'}
          </label>
        ))}
      </Section>

      <Section title="MODEL ROUTE">
        {ROUTES.map((r) => (
          <label key={r.id} className="block">
            <input
              type="radio"
              name="s-route"
              checked={s.route === r.id}
              onChange={() => void attempt(() => save({ route: r.id }))}
            />{' '}
            {r.label}
          </label>
        ))}
        <label className="mt-2 block">
          Model
          <select
            aria-label="Model"
            value={s.model ?? view.default_model}
            onChange={(e) => void attempt(() => save({ model: e.target.value }))}
            className="ml-2 border border-white/15 bg-[#04090f] px-1 py-0.5"
          >
            {view.models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
        </label>
        {(['anthropic', 'openrouter'] as const).map((r) => (
          <div key={r} className="mt-2">
            <p>
              {r === 'anthropic' ? 'Anthropic' : 'OpenRouter'} key:{' '}
              {view.keys[r] ? 'saved in keychain' : 'not set'}
              {view.keys[r] ? (
                <button
                  type="button"
                  className="hud-btn ml-2 py-0"
                  onClick={() =>
                    void attempt(async () => {
                      await call('delete_secret', { route: r });
                      await reload();
                    })
                  }
                >
                  {r === 'anthropic' ? 'Remove Anthropic key' : 'Remove OpenRouter key'}
                </button>
              ) : null}
            </p>
            {!view.keys[r] ? (
              <KeyField
                route={r}
                label={`${r === 'anthropic' ? 'Anthropic' : 'OpenRouter'} API key`}
                onSaved={reload}
              />
            ) : null}
          </div>
        ))}
      </Section>

      <Section title="PUSH TO TALK">
        <p>Hold {s.hotkey} to talk.</p>
        <input
          aria-label="New hotkey"
          placeholder="e.g. Alt+Space"
          value={hotkey}
          onChange={(e) => setHotkey(e.target.value)}
          className="mt-1 border border-white/15 bg-transparent px-2 py-0.5"
        />
        <button
          type="button"
          className="hud-btn ml-2 py-0.5"
          onClick={() => void attempt(() => save({ hotkey: hotkey.trim() }))}
        >
          Change
        </button>
      </Section>

      <Section title="FOLDERS MARKON MAY USE">
        <ul className="space-y-1">
          {s.approved_folders.map((f) => (
            <li key={f} className="flex items-center justify-between gap-2">
              <span className="truncate">{f}</span>
              <button
                type="button"
                aria-label={`Remove ${f}`}
                className="hud-btn py-0"
                onClick={() =>
                  void attempt(() => save({ approved_folders: s.approved_folders.filter((x) => x !== f) }))
                }
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        <button
          type="button"
          className="hud-btn mt-2"
          onClick={() =>
            void attempt(async () => {
              const f = await call<string | null>('pick_folder');
              if (f && !s.approved_folders.includes(f))
                await save({ approved_folders: [...s.approved_folders, f] });
            })
          }
        >
          Add folder
        </button>
      </Section>

      <Section title="SOURCES">
        <SourcesSection view={view} save={save} reload={reload} attempt={attempt} />
      </Section>

      <Section title="AGENTS AND SKILLS">
        <AutomationSection view={view} save={save} reload={reload} attempt={attempt} />
      </Section>

      <Section title="INTEGRATIONS">
        {view.mcp.length ? (
          view.mcp.map((m) => (
            <p key={m.name}>
              {m.name}: {m.status.state.replaceAll('_', ' ')}
            </p>
          ))
        ) : (
          <p>No MCP servers in ~/.markon/mcp.json.</p>
        )}
      </Section>

      <Section title="NETWORK">
        <ul className="space-y-1">
          {view.endpoints.map((e) => (
            <li key={e.name}>
              <span className={e.on ? 'text-primary' : 'text-white/40'}>{e.on ? '● on' : '○ off'}</span>{' '}
              {e.name}: {e.host}
              <span className="block text-white/45">{e.note}</span>
            </li>
          ))}
        </ul>
      </Section>

      <Section title="USAGE TODAY">
        {view.usage_today.length ? (
          view.usage_today.map((u) => (
            <p key={u.provider + u.model}>
              {u.model}: {nf.format(u.tokens_in)} in · {nf.format(u.tokens_out)} out
            </p>
          ))
        ) : (
          <p>Nothing yet today.</p>
        )}
      </Section>

      <Section title="MOTION">
        <label>
          <input
            type="checkbox"
            checked={s.reduced_motion}
            onChange={(e) => void attempt(() => save({ reduced_motion: e.target.checked }))}
          />{' '}
          Reduce motion
        </label>
      </Section>

      <Section title="MEMORY AND DATA">
        <p className="text-white/50">Everything lives in {view.data_dir}. Keys live in the macOS keychain.</p>
        <button
          type="button"
          className="hud-btn mt-2"
          onClick={() => void attempt(async () => setExported((await call<string>('export_memory')) ?? ''))}
        >
          Export memory
        </button>
        {exported ? <p className="mt-1 break-all">Saved to {exported}</p> : null}
        <div className="mt-2">
          {wiping ? (
            <button
              type="button"
              className="hud-btn border-red-400/70 text-red-300"
              onClick={() => void attempt(() => call('wipe_all'))}
            >
              Yes, wipe everything
            </button>
          ) : (
            <button type="button" className="hud-btn" onClick={() => setWiping(true)}>
              Wipe everything
            </button>
          )}
        </div>
      </Section>
    </div>
  );
}
