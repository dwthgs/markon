import { useState } from 'react';
import type { Route } from '@/lib/settings';
import { call } from '@/lib/tauri';

/** Paste a key: it goes straight to the keychain and the field is cleared. Then the key is checked. */
export function KeyField({ route, label, onSaved }: { route: Route; label: string; onSaved?: () => void }) {
  const [value, setValue] = useState('');
  const [note, setNote] = useState('');
  const save = async () => {
    const v = value.trim();
    setValue('');
    if (!v) return;
    try {
      await call('set_secret', { route, value: v });
      setNote('Saved. Checking…');
      const acct = await call<{ plan: string | null; balance: string | null }>('validate_route', { route });
      setNote(`Key works${acct?.plan ? ` · ${acct.plan}` : ''}${acct?.balance ? ` · ${acct.balance}` : ''}`);
      onSaved?.();
    } catch (e) {
      setNote(`That key did not work: ${String(e)}`);
    }
  };
  return (
    <div className="mt-2">
      <label className="block text-[10px] tracking-widest text-white/55">
        {label}
        <input
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && void save()}
          className="mt-1 block w-full border border-white/15 bg-transparent px-2 py-1 font-mono text-xs text-white outline-none focus:border-primary/60"
        />
      </label>
      <button type="button" className="hud-btn mt-2" onClick={() => void save()}>
        Save key
      </button>
      {note ? <p className="mt-1 text-[11px] text-white/70">{note}</p> : null}
    </div>
  );
}
