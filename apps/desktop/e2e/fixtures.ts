import type { Page } from '@playwright/test';

export const settingsView = {
  settings: {
    voice: null,
    route: 'anthropic',
    model: null,
    hotkey: 'Alt+Space',
    approved_folders: [],
    onboarded: false,
    reduced_motion: false,
    device_id: 'abc',
    relay_url: '',
    sources: {
      imessage: false,
      calendar: false,
      notes: false,
      github: false,
      mail: [],
      mcp: [],
      telegram: false,
      telegram_api_id: null,
      slack_send_tool: null,
    },
    watch: { enabled: false, people: [], words: ['urgent', 'asap', 'emergency'] },
  },
  keys: { anthropic: false, openrouter: false },
  models: [
    { id: 'claude-opus-5-5', name: 'Claude Opus 5.5' },
    { id: 'claude-sonnet-5-5', name: 'Claude Sonnet 5.5' },
  ],
  default_model: 'claude-opus-5-5',
  mcp: [],
  verified_mcp: {
    servers: [
      {
        id: 'github',
        name: 'GitHub',
        available: false,
        reason: 'needs GitHub sign-in (OAuth), not wired yet',
      },
      { id: 'gmail', name: 'Gmail', available: false, reason: 'no verified web-hosted server' },
    ],
  },
  automation: {
    agents_dir: '/Users/me/.markon/agents',
    skills_dir: '/Users/me/.markon/skills',
    agents: [{ name: 'Morning briefing', schedule: '30 7 * * *', tools: ['what_did_i_miss'], enabled: true }],
    agent_problems: ['/Users/me/.markon/agents/broken.toml: unknown field `schedul`'],
    skills: [
      { name: 'markon-setup', description: 'How to connect sources.', trust: 'bundled' },
      { name: 'trip-planner', description: 'Plan a trip.', trust: 'unreviewed' },
    ],
    skill_problems: [],
  },
  sources: {
    full_disk_access: false,
    github_token: false,
    mail_passwords: [],
    approvals: [['send_imessage', 'sara']],
  },
  usage_today: [{ provider: 'anthropic', model: 'claude-opus-5-5', tokens_in: 1200, tokens_out: 300 }],
  endpoints: [{ name: 'Model', host: 'api.anthropic.com', on: true, note: 'Needed to answer you.' }],
  data_dir: '/Users/me/.markon',
};

export const open = async (page: Page, query = '', responses: Record<string, unknown> = {}) => {
  await page.addInitScript((r) => {
    (window as unknown as { __e2eResponses: unknown }).__e2eResponses = r;
  }, responses);
  await page.goto(`/?e2e${query}`);
  await page.waitForFunction(() => window.__e2e !== undefined);
};

export const prime = (page: Page, responses: Record<string, unknown>) =>
  page.evaluate((r) => Object.assign(window.__e2e.responses, r), responses);

export const calls = (page: Page, cmd: string) =>
  page.evaluate((c) => window.__e2e.calls.filter((x) => x.cmd === c).map((x) => x.args), cmd);
