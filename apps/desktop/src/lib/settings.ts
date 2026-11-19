import { useCallback, useEffect, useState } from 'react';
import { call } from './tauri';

export type Route = 'anthropic' | 'openrouter' | 'relay' | 'local' | 'builtin' | 'claudecode';
export type Voice = 'markon' | 'markan';
export type VoiceEngine = 'kokoro' | 'cartesia';

export interface SettingsView {
  settings: {
    voice: Voice | null;
    voice_engine: VoiceEngine;
    route: Route;
    model: string | null;
    hotkey: string;
    approved_folders: string[];
    onboarded: boolean;
    reduced_motion: boolean;
    device_id: string | null;
    relay_url: string;
    sources: SourceSettings;
    watch: { enabled: boolean; people: string[]; words: string[] };
    local_url: string;
    local_model: string | null;
  };
  local: {
    ram_gb: number;
    fit: string[];
    model: string;
    builtin_ready: boolean;
    builtin_mb: number;
    chunks: number;
    meaning_search: boolean;
  };
  keys: { anthropic: boolean; openrouter: boolean; cartesia: boolean };
  models: { id: string; name: string }[];
  default_model: string;
  mcp: { name: string; status: { state: string } }[];
  verified_mcp: { servers: { id: string; name: string; available: boolean; reason: string }[] };
  automation: {
    agents_dir: string;
    skills_dir: string;
    agents: { name: string; schedule: string | null; tools: string[]; enabled: boolean }[];
    agent_problems: string[];
    skills: { name: string; description: string; trust: 'bundled' | 'yours' | 'unreviewed' }[];
    skill_problems: string[];
  };
  traces: {
    id: number;
    user_text: string;
    model: string;
    stt_ms: number | null;
    first_text_ms: number | null;
    first_audio_ms: number | null;
    total_ms: number;
    tools: { name: string; ms: number; ok: boolean }[];
    ok: boolean;
  }[];
  proposals: { path: string; diff: string }[];
  sources: {
    full_disk_access: boolean;
    github_token: boolean;
    mail_passwords: boolean[];
    approvals: [string, string][];
  };
  usage_today: { provider: string; model: string; tokens_in: number; tokens_out: number }[];
  endpoints: { name: string; host: string; on: boolean; note: string }[];
  data_dir: string;
}

export interface MailAccount {
  host: string;
  port: number;
  user: string;
  enabled: boolean;
}

/** Mirrors `sources::SourceSettings`: everything is off until switched on. */
export interface SourceSettings {
  imessage: boolean;
  calendar: boolean;
  notes: boolean;
  github: boolean;
  mail: MailAccount[];
  mcp: { server: string; tool: string; args: unknown }[];
  telegram: boolean;
  telegram_api_id: number | null;
  slack_send_tool: string | null;
  private: string[];
}

export type Patch = Partial<Omit<SettingsView['settings'], 'device_id' | 'relay_url'>>;

export const ROUTES: { id: Route; label: string; hint: string }[] = [
  {
    id: 'claudecode',
    label: 'Claude Code on this Mac',
    hint: 'No key: uses the Claude plan Claude Code is signed in with.',
  },
  { id: 'anthropic', label: 'My own Anthropic key', hint: 'Claude, billed to your Anthropic account.' },
  {
    id: 'openrouter',
    label: 'My own OpenRouter key',
    hint: 'A curated list of models, billed to OpenRouter.',
  },
  { id: 'relay', label: 'Free trial', hint: 'A few messages a day through Markon’s relay.' },
];

/** On this Mac, no cloud. Settings only: onboarding keeps to the hosted routes. */
export const LOCAL_ROUTES: { id: Route; label: string; hint: string }[] = [
  {
    id: 'local',
    label: 'A server on this Mac: Ollama or LM Studio (experimental)',
    hint: 'Ollama or LM Studio, which you run yourself.',
  },
  {
    id: 'builtin',
    label: 'Built into Markon, no server (experimental)',
    hint: 'Qwen2.5 1.5B inside Markon. No server, no cloud.',
  },
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
