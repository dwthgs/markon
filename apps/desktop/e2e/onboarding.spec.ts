import { expect, test } from '@playwright/test';
import { calls, open, settingsView } from './fixtures';

/* Onboarding: voice, route, microphone, folders, integrations, done. */
test('first launch walks through voice, route, mic, folders and integrations', async ({ page }) => {
  await open(page, '&onboarded=false&has_key=false', {
    settings_get: settingsView,
    validate_route: { route: 'anthropic', plan: null, balance: null },
    request_mic: true,
    pick_folder: '/Users/me/Documents',
  });
  const ob = page.getByRole('dialog', { name: /set up markon/i });
  await expect(ob).toBeVisible();

  await ob.getByRole('button', { name: 'Hear Markon' }).click();
  await ob.getByRole('button', { name: 'Hear Markan' }).click();
  expect(await calls(page, 'preview_voice')).toEqual([{ voice: 'markon' }, { voice: 'markan' }]);
  await ob.getByRole('radio', { name: /markan/i }).check();
  await ob.getByRole('button', { name: 'Next' }).click();

  await expect(ob.getByRole('radio', { name: /own anthropic key/i })).toBeChecked();
  const key = ob.getByLabel(/anthropic api key/i);
  await expect(key).toHaveAttribute('type', 'password');
  await key.fill('sk-ant-test-value');
  await ob.getByRole('button', { name: 'Save key' }).click();
  await expect(ob).toContainText(/key works/i);
  expect(await calls(page, 'set_secret')).toEqual([{ route: 'anthropic', value: 'sk-ant-test-value' }]);
  await expect(key).toHaveValue('');
  await ob.getByRole('button', { name: 'Next' }).click();

  await ob.getByRole('button', { name: 'Allow microphone' }).click();
  await expect(ob).toContainText(/microphone ready/i);
  await ob.getByRole('button', { name: 'Next' }).click();

  await ob.getByRole('button', { name: 'Add folder' }).click();
  await expect(ob).toContainText('/Users/me/Documents');
  await ob.getByRole('button', { name: 'Next' }).click();

  await expect(ob).toContainText('Gmail');
  await expect(ob).toContainText(/no verified web-hosted server/i);
  await ob.getByRole('button', { name: 'Finish' }).click();

  expect((await calls(page, 'settings_set')).at(-1)).toEqual({
    patch: {
      voice: 'markan',
      route: 'anthropic',
      approved_folders: ['/Users/me/Documents'],
      onboarded: true,
    },
  });
  await expect(ob).toBeHidden();
});

test('the free trial explains when the relay is not available', async ({ page }) => {
  await open(page, '&onboarded=false', { settings_get: settingsView });
  const ob = page.getByRole('dialog', { name: /set up markon/i });
  await ob.getByRole('button', { name: 'Next' }).click();
  await ob.getByRole('radio', { name: /free trial/i }).check();
  await expect(ob).toContainText(/not available in this build/i);
});

test('onboarding is skipped once done', async ({ page }) => {
  await open(page);
  await expect(page.getByRole('dialog', { name: /set up markon/i })).toHaveCount(0);
});
