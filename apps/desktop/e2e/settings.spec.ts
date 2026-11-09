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
