import { useCallback, useEffect, useState } from 'react';
import { call } from './tauri';

export type Route = 'anthropic' | 'openrouter' | 'relay';
export type Voice = 'markon' | 'markan';

export interface SettingsView {
  settings: {
    voice: Voice | null;
    route: Route;
    model: string | null;
    hotkey: string;
    approved_folders: string[];
    onboarded: boolean;
    reduced_motion: boolean;
    device_id: string | null;
    relay_url: string;
  };
  keys: { anthropic: boolean; openrouter: boolean };
  models: { id: string; name: string }[];
  default_model: string;
  mcp: { name: string; status: { state: string } }[];
  verified_mcp: { servers: { id: string; name: string; available: boolean; reason: string }[] };
  usage_today: { provider: string; model: string; tokens_in: number; tokens_out: number }[];
  endpoints: { name: string; host: string; on: boolean; note: string }[];
  data_dir: string;
}

export type Patch = Partial<Omit<SettingsView['settings'], 'device_id' | 'relay_url'>>;

export const ROUTES: { id: Route; label: string; hint: string }[] = [
  { id: 'anthropic', label: 'My own Anthropic key', hint: 'Claude, billed to your Anthropic account.' },
  {
    id: 'openrouter',
    label: 'My own OpenRouter key',
    hint: 'A curated list of models, billed to OpenRouter.',
  },
  { id: 'relay', label: 'Free trial', hint: 'A few messages a day through Markon’s relay.' },
];

export function useSettings() {
  const [view, setView] = useState<SettingsView | null>(null);
  const reload = useCallback(async () => setView((await call<SettingsView>('settings_get')) ?? null), []);
  useEffect(() => {
    void reload();
  }, [reload]);
  const save = useCallback(
    async (patch: Patch) => {
      await call('settings_set', { patch });
      await reload();
    },
    [reload],
  );
  return { view, reload, save };
}
