import { useState } from 'react';
import { ROUTES, type Route, useSettings, type Voice } from '@/lib/settings';
import { call } from '@/lib/tauri';
import { KeyField } from './KeyField';

const STEPS = ['Voice', 'Model', 'Microphone', 'Folders', 'Integrations'] as const;

/** First launch: voice, route, microphone, folders, integrations. Under two minutes. */
export function Onboarding({ onDone }: { onDone: () => void }) {
  const { view, save } = useSettings();
  const [step, setStep] = useState(0);
  const [voice, setVoice] = useState<Voice>('markon');
  const [route, setRoute] = useState<Route>('anthropic');
  const [mic, setMic] = useState<'unknown' | 'ok' | 'denied'>('unknown');
  const [folders, setFolders] = useState<string[]>([]);

  const finish = async () => {
    await save({ voice, route, approved_folders: folders, onboarded: true });
    onDone();
  };

  return (
    <div className="fixed inset-0 z-40 grid place-items-center bg-black/60">
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Set up Markon"
        className="w-[min(560px,92vw)] border border-primary/60 bg-[#04090f] px-7 py-6 font-mono text-xs text-white/75 shadow-[0_0_28px_rgba(0,210,235,.25)]"
      >
        <p className="text-[10px] tracking-[.3em] text-white/45">
          SET UP · {step + 1}/{STEPS.length} · {STEPS[step]?.toUpperCase()}
        </p>

        {step === 0 ? (
          <section className="mt-4">
            <h2 className="font-heading text-sm tracking-[.2em] text-primary">CHOOSE A VOICE</h2>
            <p className="mt-2">
              Pick once. The voice stays the same on every Mac and never changes without you.
            </p>
            <div className="mt-4 grid grid-cols-2 gap-3">
              {(['markon', 'markan'] as const).map((v) => (
                <div key={v} className="border border-white/15 p-3">
                  <label className="flex items-center gap-2 text-sm text-white">
                    <input type="radio" name="voice" checked={voice === v} onChange={() => setVoice(v)} />
                    {v === 'markon' ? 'Markon (male)' : 'Markan (female)'}
                  </label>
                  <button
                    type="button"
                    className="hud-btn mt-2"
                    onClick={() => void call('preview_voice', { voice: v })}
                  >
                    {v === 'markon' ? 'Hear Markon' : 'Hear Markan'}
                  </button>
                </div>
              ))}
            </div>
          </section>
        ) : null}

        {step === 1 ? (
          <section className="mt-4">
            <h2 className="font-heading text-sm tracking-[.2em] text-primary">HOW SHOULD MARKON THINK?</h2>
            <div className="mt-3 space-y-2">
              {ROUTES.map((r) => (
                <label key={r.id} className="flex items-start gap-2">
                  <input type="radio" name="route" checked={route === r.id} onChange={() => setRoute(r.id)} />
                  <span>
                    <span className="text-white">{r.label}</span>
                    <span className="block text-white/50">{r.hint}</span>
                  </span>
                </label>
              ))}
            </div>
            {route === 'anthropic' ? <KeyField route="anthropic" label="Anthropic API key" /> : null}
            {route === 'openrouter' ? <KeyField route="openrouter" label="OpenRouter API key" /> : null}
            {route === 'relay' && !view?.settings.relay_url ? (
              <p className="mt-3 text-amber-300/90">
                The free trial is not available in this build yet. Use your own key for now.
              </p>
            ) : null}
          </section>
        ) : null}

        {step === 2 ? (
          <section className="mt-4">
            <h2 className="font-heading text-sm tracking-[.2em] text-primary">MICROPHONE</h2>
            <p className="mt-2">Markon listens only while you hold {'⌥'} Space. Audio stays on this Mac.</p>
            <button
              type="button"
              className="hud-btn mt-3"
              onClick={async () => setMic((await call<boolean>('request_mic')) ? 'ok' : 'denied')}
            >
              Allow microphone
            </button>
            {mic === 'ok' ? <p className="mt-2 text-primary">Microphone ready.</p> : null}
            {mic === 'denied' ? (
              <p className="mt-2 text-amber-300/90">
                macOS blocked the microphone.{' '}
                <button type="button" className="underline" onClick={() => void call('open_mic_settings')}>
                  Open microphone settings
                </button>{' '}
                or type instead.
              </p>
            ) : null}
          </section>
        ) : null}

        {step === 3 ? (
          <section className="mt-4">
            <h2 className="font-heading text-sm tracking-[.2em] text-primary">FOLDERS MARKON MAY USE</h2>
            <p className="mt-2">Markon reads only inside these folders and asks before writing.</p>
            <ul className="mt-2 space-y-1">
              {folders.map((f) => (
                <li key={f} className="flex items-center justify-between border border-white/10 px-2 py-1">
                  <span className="truncate">{f}</span>
                  <button
                    type="button"
                    className="hud-btn py-0"
                    onClick={() => setFolders(folders.filter((x) => x !== f))}
                  >
                    Remove
                  </button>
                </li>
              ))}
            </ul>
            <button
              type="button"
              className="hud-btn mt-3"
              onClick={async () => {
                const f = await call<string | null>('pick_folder');
                if (f && !folders.includes(f)) setFolders([...folders, f]);
              }}
            >
              Add folder
            </button>
          </section>
        ) : null}

        {step === 4 ? (
          <section className="mt-4">
            <h2 className="font-heading text-sm tracking-[.2em] text-primary">INTEGRATIONS (OPTIONAL)</h2>
            <ul className="mt-3 space-y-1">
              {(view?.verified_mcp.servers ?? []).map((s) => (
                <li key={s.id}>
                  <span className="text-white">{s.name}</span> · {s.available ? 'available' : s.reason}
                </li>
              ))}
            </ul>
            <p className="mt-3 text-white/50">
              Other MCP servers can be added to ~/.markon/mcp.json, the same format as Claude Desktop.
            </p>
          </section>
        ) : null}

        <div className="mt-6 flex justify-between">
          <button type="button" className="hud-btn" disabled={step === 0} onClick={() => setStep(step - 1)}>
            Back
          </button>
          {step < STEPS.length - 1 ? (
            <button type="button" className="hud-btn hud-btn-on" onClick={() => setStep(step + 1)}>
              Next
            </button>
          ) : (
            <button type="button" className="hud-btn hud-btn-on" onClick={() => void finish()}>
              Finish
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
