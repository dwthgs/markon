import { expect, test } from '@playwright/test';
import { calls, open, settingsView } from './fixtures';

const onboarded = {
  ...settingsView,
  settings: {
    ...settingsView.settings,
    onboarded: true,
    voice: 'markon',
    approved_folders: ['/Users/me/Notes'],
  },
  keys: { anthropic: true, openrouter: false },
};

test.beforeEach(async ({ page }) => {
  await open(page, '', {
    settings_get: onboarded,
    pick_folder: '/Users/me/Work',
    export_memory: '/Users/me/.markon/markon-export-1.json',
  });
  await page.evaluate(() => window.__e2e.emit('markon://menu', 'settings'));
});

test('Cmd+, opens settings in the HUD and Esc closes it', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await expect(panel).toBeVisible();
  await expect(panel).toContainText('api.anthropic.com');
  await expect(panel).toContainText('1,200 in · 300 out');
  await page.keyboard.press('Escape');
  await expect(panel).toBeHidden();
  expect(await calls(page, 'hide_window')).toHaveLength(0);
});

test('the Close button closes settings', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await panel.getByRole('button', { name: /close/i }).click();
  await expect(panel).toBeHidden();
});

test('switching route and model saves straight away', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await panel.getByLabel('Model').selectOption('claude-sonnet-5-5');
  expect((await calls(page, 'settings_set')).at(-1)).toEqual({ patch: { model: 'claude-sonnet-5-5' } });
  // The fake core does not persist, so the radio stays controlled: click, then read the saved patch.
  await panel.getByRole('radio', { name: /openrouter/i }).click();
  expect((await calls(page, 'settings_set')).at(-1)).toEqual({ patch: { route: 'openrouter' } });
});

test('keys are stored in the keychain and can be removed', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await expect(panel).toContainText(/anthropic key: saved in keychain/i);
  await panel.getByRole('button', { name: 'Remove Anthropic key' }).click();
  expect(await calls(page, 'delete_secret')).toEqual([{ route: 'anthropic' }]);
});

test('folders can be added and removed', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await panel.getByRole('button', { name: 'Add folder' }).click();
  expect((await calls(page, 'settings_set')).at(-1)).toEqual({
    patch: { approved_folders: ['/Users/me/Notes', '/Users/me/Work'] },
  });
  await panel.getByRole('button', { name: 'Remove /Users/me/Notes' }).click();
  // The fake core does not persist the addition, so removing from the original list leaves it empty.
  expect((await calls(page, 'settings_set')).at(-1)).toEqual({ patch: { approved_folders: [] } });
});

test('export and a confirmed full wipe', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await panel.getByRole('button', { name: 'Export memory' }).click();
  await expect(panel).toContainText('markon-export-1.json');
  await panel.getByRole('button', { name: 'Wipe everything' }).click();
  expect(await calls(page, 'wipe_all')).toHaveLength(0);
  await panel.getByRole('button', { name: 'Yes, wipe everything' }).click();
  expect(await calls(page, 'wipe_all')).toHaveLength(1);
});

test('double-clicking the drag strip toggles full screen', async ({ page }) => {
  await page.keyboard.press('Escape');
  await page.getByTestId('drag-strip').dblclick({ position: { x: 20, y: 10 } });
  expect(await calls(page, 'toggle_full_screen')).toHaveLength(1);
});

test('sources start off, and switching one on asks for Full Disk Access when it is missing', async ({
  page,
}) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  const messages = panel.getByRole('checkbox', { name: 'Messages', exact: true });
  await expect(messages).not.toBeChecked();
  await expect(panel).not.toContainText('need Full Disk Access');
  await messages.click();
  expect((await calls(page, 'settings_set')).at(-1)).toEqual({
    patch: { sources: { ...onboarded.settings.sources, imessage: true } },
  });
});

test('a mail account is added with its app password going to the keychain', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await panel.getByLabel('Mail server').fill('imap.gmail.com');
  await panel.getByLabel('Mail address').fill('me@gmail.com');
  await panel.getByLabel('App password').fill('abcd efgh ijkl mnop');
  await panel.getByRole('button', { name: 'Add mail account' }).click();
  expect(await calls(page, 'set_source_secret')).toEqual([
    { kind: 'mail', user: 'me@gmail.com', host: 'imap.gmail.com', value: 'abcd efgh ijkl mnop' },
  ]);
  const patch = (await calls(page, 'settings_set')).at(-1) as { patch: { sources: { mail: unknown[] } } };
  expect(patch.patch.sources.mail).toEqual([
    { host: 'imap.gmail.com', port: 993, user: 'me@gmail.com', enabled: true },
  ]);
  expect(JSON.stringify(patch)).not.toContain('abcd');
});

test('agent files and skills are listed, a broken file says why, an unreviewed skill can be approved', async ({
  page,
}) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await expect(panel).toContainText('Morning briefing · 30 7 * * *');
  await expect(panel).toContainText('unknown field `schedul`');
  await expect(panel.getByRole('button', { name: 'Approve markon-setup' })).toHaveCount(0);
  await panel.getByRole('button', { name: 'Approve trip-planner' }).click();
  expect(await calls(page, 'approve_skill')).toEqual([{ name: 'trip-planner' }]);
});

test('the watch agent starts off and is switched on in settings', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  const box = panel.getByRole('checkbox', { name: /speak up when something important arrives/i });
  await expect(box).not.toBeChecked();
  await box.click();
  expect((await calls(page, 'settings_set')).at(-1)).toEqual({
    patch: { watch: { enabled: true, people: [], words: ['urgent', 'asap', 'emergency'] } },
  });
});

test('remembered always-yes answers are listed and can be forgotten', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await expect(panel).toContainText('send imessage to sara');
  await panel.getByRole('button', { name: /forget every "always yes"/i }).click();
  expect(await calls(page, 'forget_approvals')).toHaveLength(1);
});

test('the local routes are offered in settings, marked experimental, with models that fit', async ({
  page,
}) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await panel
    .getByRole('radio', { name: /a server on this mac: ollama or lm studio \(experimental\)/i })
    .click();
  expect((await calls(page, 'settings_set')).at(-1)).toEqual({ patch: { route: 'local' } });
  await expect(
    panel.getByRole('radio', { name: /built into markon, no server \(experimental\)/i }),
  ).toBeVisible();
});

test('a source can be kept on this Mac', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await panel.getByRole('checkbox', { name: 'Messages private' }).click();
  const patch = (await calls(page, 'settings_set')).at(-1) as { patch: { sources: { private: string[] } } };
  expect(patch.patch.sources.private).toEqual(['imessage']);
  await expect(panel.getByRole('button', { name: 'Download the built-in model' })).toBeHidden();
});

test('the folders section says how much is indexed and offers meaning search', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  await expect(panel).toContainText('342 passages indexed');
  await panel.getByRole('button', { name: 'Add meaning search (23 MB)' }).click();
  expect(await calls(page, 'download_embedder')).toHaveLength(1);
});

test('where the time went, and a proposed change that waits for a yes', async ({ page }) => {
  const panel = page.getByRole('dialog', { name: 'Settings' });
  const row = panel.getByRole('row', { name: /what did I miss/ });
  await expect(row).toContainText('350 ms');
  await expect(row).toContainText('4.1 s');
  await expect(panel).toContainText('+ prompt = "new"');
  await panel.getByRole('button', { name: 'Approve change' }).click();
  expect(await calls(page, 'approve_proposal')).toEqual([{ path: '/Users/me/.markon/agents/standup.toml' }]);
  await panel.getByRole('button', { name: 'Suggest improvements' }).click();
  expect(await calls(page, 'suggest_improvements')).toHaveLength(1);
});
