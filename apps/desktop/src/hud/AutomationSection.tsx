import { useState } from 'react';
import type { Patch, SettingsView } from '@/lib/settings';
import { call } from '@/lib/tauri';

/** Agents and skills are files; the watch agent is a list of people and words. */
export function AutomationSection({
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
  const a = view.automation;
  const watch = view.settings.watch;
  const [people, setPeople] = useState(watch.people.join(', '));
  const list = (s: string) =>
    s
      .split(',')
      .map((x) => x.trim())
      .filter(Boolean);
  return (
    <div className="space-y-2">
      <div>
        <p className="text-white/50">Agents: drop a .toml file in {a.agents_dir}. No restart needed.</p>
        <ul>
          {a.agents.map((g) => (
            <li key={g.name}>
              {g.enabled ? '●' : '○'} {g.name} {g.schedule ? `· ${g.schedule}` : ''}{' '}
              <span className="text-white/45">· {g.tools.join(', ') || 'no tools'}</span>
            </li>
          ))}
        </ul>
        {a.agent_problems.map((p) => (
          <p key={p} className="text-amber-300">
            {p}
          </p>
        ))}
      </div>
      <div>
        <p className="text-white/50">Skills: a folder with a SKILL.md in {a.skills_dir}.</p>
        <ul>
          {a.skills.map((s) => (
            <li key={s.name} className="flex items-center justify-between gap-2">
              <span className="truncate">
                {s.name} <span className="text-white/45">· {s.trust}</span>
              </span>
              {s.trust === 'unreviewed' ? (
                <button
                  type="button"
                  className="hud-btn py-0"
                  onClick={() =>
                    void attempt(async () => {
                      await call('approve_skill', { name: s.name });
                      await reload();
                    })
                  }
                >
                  Approve {s.name}
                </button>
              ) : null}
            </li>
          ))}
        </ul>
        {a.skill_problems.map((p) => (
          <p key={p} className="text-amber-300">
            {p}
          </p>
        ))}
      </div>
      <div>
        <label>
          <input
            type="checkbox"
            checked={watch.enabled}
            onChange={(e) => void attempt(() => save({ watch: { ...watch, enabled: e.target.checked } }))}
          />{' '}
          Speak up when something important arrives (checks every five minutes, quiet during calls)
        </label>
        <div className="mt-1">
          <input
            aria-label="Watched people"
            placeholder="People, comma separated"
            value={people}
            onChange={(e) => setPeople(e.target.value)}
            onBlur={() => void attempt(() => save({ watch: { ...watch, people: list(people) } }))}
            className="w-64 border border-white/15 bg-transparent px-2 py-0.5"
          />
          <p className="text-white/45">Words: {watch.words.join(', ')}</p>
        </div>
      </div>
    </div>
  );
}
