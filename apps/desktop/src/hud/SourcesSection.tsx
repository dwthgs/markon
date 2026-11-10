import { useEffect, useState } from 'react';
import type { Patch, SettingsView, SourceSettings } from '@/lib/settings';
import { call } from '@/lib/tauri';

const MAC: { id: 'imessage' | 'calendar' | 'notes'; label: string }[] = [
  { id: 'imessage', label: 'Messages' },
  { id: 'calendar', label: 'Calendar' },
  { id: 'notes', label: 'Notes' },
];

/** What Markon may read. Everything starts off; secrets go to the keychain, never settings. */
export function SourcesSection({
  view,
  save,
  reload,
  attempt,
}: {
  view: SettingsView;
  save: (p: Patch) => Promise<void>;
  reload: () => Promise<void>;
  attempt: (f: () => Promise<unknown>) => Promise<void>;
}) {
  const src = view.settings.sources;
  const set = (p: Partial<SourceSettings>) => attempt(() => save({ sources: { ...src, ...p } }));
  const [token, setToken] = useState('');
  const [mail, setMail] = useState({ host: '', user: '', password: '' });
  const macOn = src.imessage || src.calendar || src.notes;
  const oauth = view.mcp.filter((m) => m.status.state === 'needs_sign_in' || m.status.state === 'online');

  return (
    <div className="space-y-2">
      <p className="text-white/50">Off until you switch a source on. Markon only reads; it never sends.</p>
      {MAC.map((m) => (
        <label key={m.id} className="mr-4">
          <input
            type="checkbox"
            checked={src[m.id]}
            onChange={(e) => void set({ [m.id]: e.target.checked })}
          />{' '}
          {m.label}
        </label>
      ))}
      {macOn && !view.sources.full_disk_access ? (
        <p className="text-amber-300">
          Messages, Calendar and Notes need Full Disk Access for Markon.{' '}
          <button type="button" className="hud-btn py-0" onClick={() => void call('open_full_disk_access')}>
            Open Full Disk Access settings
          </button>
        </p>
      ) : null}

      <div>
        <label>
          <input
            type="checkbox"
            checked={src.github}
            onChange={(e) => void set({ github: e.target.checked })}
          />{' '}
          GitHub notifications {view.sources.github_token ? '(token saved in keychain)' : ''}
        </label>
        {src.github && !view.sources.github_token ? (
          <div className="mt-1">
            <input
              aria-label="GitHub token"
              type="password"
              placeholder="Personal access token (notifications scope)"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              className="w-64 border border-white/15 bg-transparent px-2 py-0.5"
            />
            <button
              type="button"
              className="hud-btn ml-2 py-0.5"
              onClick={() =>
                void attempt(async () => {
                  await call('set_source_secret', { kind: 'github', value: token });
                  setToken('');
                  await reload();
                })
              }
            >
              Save token
            </button>
          </div>
        ) : null}
      </div>

      <div>
        <p>Mail (IMAP with an app password)</p>
        <ul className="space-y-1">
          {src.mail.map((a, i) => (
            <li key={`${a.user}@${a.host}`} className="flex items-center justify-between gap-2">
              <label className="truncate">
                <input
                  type="checkbox"
                  checked={a.enabled}
                  onChange={(e) =>
                    void set({
                      mail: src.mail.map((x, j) => (j === i ? { ...x, enabled: e.target.checked } : x)),
                    })
                  }
                />{' '}
                {a.user} · {a.host} {view.sources.mail_passwords[i] ? '' : '(no app password)'}
              </label>
              <button
                type="button"
                aria-label={`Remove ${a.user}`}
                className="hud-btn py-0"
                onClick={() =>
                  void attempt(async () => {
                    await call('delete_source_secret', { kind: 'mail', user: a.user, host: a.host });
                    await save({ sources: { ...src, mail: src.mail.filter((_, j) => j !== i) } });
                  })
                }
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        <div className="mt-1 flex flex-wrap gap-1">
          <input
            aria-label="Mail server"
            placeholder="imap.gmail.com"
            value={mail.host}
            onChange={(e) => setMail({ ...mail, host: e.target.value })}
            className="w-32 border border-white/15 bg-transparent px-2 py-0.5"
          />
          <input
            aria-label="Mail address"
            placeholder="you@gmail.com"
            value={mail.user}
            onChange={(e) => setMail({ ...mail, user: e.target.value })}
            className="w-36 border border-white/15 bg-transparent px-2 py-0.5"
          />
          <input
            aria-label="App password"
            type="password"
            placeholder="app password"
            value={mail.password}
            onChange={(e) => setMail({ ...mail, password: e.target.value })}
            className="w-28 border border-white/15 bg-transparent px-2 py-0.5"
          />
          <button
            type="button"
            className="hud-btn py-0.5"
            disabled={!mail.host.trim() || !mail.user.trim() || !mail.password}
            onClick={() =>
              void attempt(async () => {
                const host = mail.host.trim();
                const user = mail.user.trim();
                await call('set_source_secret', { kind: 'mail', user, host, value: mail.password });
                await save({
                  sources: { ...src, mail: [...src.mail, { host, port: 993, user, enabled: true }] },
                });
                setMail({ host: '', user: '', password: '' });
              })
            }
          >
            Add mail account
          </button>
        </div>
      </div>

      {view.sources.approvals.length ? (
        <div>
          <p>
            Always yes for:{' '}
            {view.sources.approvals.map(([tool, who]) => `${tool.replaceAll('_', ' ')} to ${who}`).join(', ')}
          </p>
          <button
            type="button"
            className="hud-btn mt-1 py-0"
            onClick={() =>
              void attempt(async () => {
                await call('forget_approvals');
                await reload();
              })
            }
          >
            Forget every "always yes"
          </button>
        </div>
      ) : null}

      <div>
        <p>Keep on this Mac (summarised by a local model; left out if there is none):</p>
        {(
          [
            ['imessage', 'Messages'],
            ['telegram', 'Telegram'],
            ...src.mail.map((a) => [`mail:${a.user}`, `Mail (${a.user})`]),
          ] as [string, string][]
        ).map(([id, label]) => (
          <label key={id} className="mr-4">
            <input
              type="checkbox"
              checked={src.private.includes(id)}
              onChange={(e) =>
                void set({
                  private: e.target.checked ? [...src.private, id] : src.private.filter((x) => x !== id),
                })
              }
            />{' '}
            {label} private
          </label>
        ))}
      </div>

      <TelegramBlock src={src} set={set} attempt={attempt} reload={reload} />

      {oauth.length ? (
        <div>
          <p>Connected services (from ~/.markon/mcp.json)</p>
          {oauth.map((m) => {
            const used = src.mcp.some((c) => c.server === m.name);
            return (
              <p key={m.name} className="flex items-center gap-2">
                <label>
                  <input
                    type="checkbox"
                    checked={used}
                    onChange={(e) =>
                      void set({
                        mcp: e.target.checked
                          ? [
                              ...src.mcp,
                              {
                                server: m.name,
                                tool: 'slack_search_public_and_private',
                                args: { query: 'is:unread' },
                              },
                            ]
                          : src.mcp.filter((c) => c.server !== m.name),
                      })
                    }
                  />{' '}
                  Read {m.name}
                </label>
                {m.status.state === 'needs_sign_in' ? (
                  <button
                    type="button"
                    className="hud-btn py-0"
                    onClick={() =>
                      void attempt(async () => {
                        await call('mcp_sign_in', { name: m.name });
                        await reload();
                      })
                    }
                  >
                    Sign in to {m.name}
                  </button>
                ) : null}
              </p>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

/** Telegram as the user: an API id and hash from my.telegram.org, then phone, code and maybe a password. */
function TelegramBlock({
  src,
  set,
  attempt,
  reload,
}: {
  src: SourceSettings;
  set: (p: Partial<SourceSettings>) => Promise<void>;
  attempt: (f: () => Promise<unknown>) => Promise<void>;
  reload: () => Promise<void>;
}) {
  const [api, setApi] = useState({ id: '', hash: '' });
  const [step, setStep] = useState<'phone' | 'code' | 'password' | 'done'>('phone');
  const [value, setValue] = useState('');
  const field = 'w-48 border border-white/15 bg-transparent px-2 py-0.5';
  useEffect(() => {
    void call<boolean>('telegram_status').then((ok) => ok && setStep('done'));
  }, []);
  const next = async () => {
    if (step === 'phone') {
      await call('telegram_request_code', { phone: value });
      setStep('code');
    } else if (step === 'code') {
      setStep((await call<boolean>('telegram_sign_in', { code: value })) ? 'password' : 'done');
    } else if (step === 'password') {
      await call('telegram_password', { password: value });
      setStep('done');
    }
    setValue('');
  };
  return (
    <div>
      <label>
        <input
          type="checkbox"
          checked={src.telegram}
          onChange={(e) => void set({ telegram: e.target.checked })}
        />{' '}
        Telegram
      </label>
      {src.telegram && src.telegram_api_id === null ? (
        <div className="mt-1 flex flex-wrap gap-1">
          <input
            aria-label="Telegram API id"
            placeholder="api_id"
            value={api.id}
            onChange={(e) => setApi({ ...api, id: e.target.value })}
            className="w-24 border border-white/15 bg-transparent px-2 py-0.5"
          />
          <input
            aria-label="Telegram API hash"
            type="password"
            placeholder="api_hash"
            value={api.hash}
            onChange={(e) => setApi({ ...api, hash: e.target.value })}
            className={field}
          />
          <button
            type="button"
            className="hud-btn py-0.5"
            onClick={() =>
              void attempt(async () => {
                await call('telegram_setup', { apiId: Number(api.id), apiHash: api.hash });
                await reload();
              })
            }
          >
            Save Telegram app
          </button>
        </div>
      ) : null}
      {src.telegram && src.telegram_api_id !== null && step !== 'done' ? (
        <div className="mt-1">
          <input
            aria-label={`Telegram ${step}`}
            type={step === 'password' ? 'password' : 'text'}
            placeholder={
              step === 'phone'
                ? '+251 91 234 5678'
                : step === 'code'
                  ? 'code from Telegram'
                  : 'two-step password'
            }
            value={value}
            onChange={(e) => setValue(e.target.value)}
            className={field}
          />
          <button type="button" className="hud-btn ml-2 py-0.5" onClick={() => void attempt(next)}>
            {step === 'phone' ? 'Send code' : 'Sign in'}
          </button>
        </div>
      ) : null}
      {step === 'done' ? <p className="text-primary">Signed in to Telegram.</p> : null}
    </div>
  );
}
