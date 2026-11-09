import { expect, type Page, test } from '@playwright/test';

/* In the HUD, with the Rust side faked. */
const open = async (page: Page, query = '') => {
  await page.goto(`/?e2e${query}`);
  await page.waitForFunction(() => window.__e2e !== undefined);
  await expect(page.getByTestId('status-line')).toBeVisible();
};
const emit = (page: Page, event: string, payload: unknown) =>
  page.evaluate(([e, p]) => window.__e2e.emit(e as string, p), [event, payload] as const);
const calls = (page: Page, cmd: string) =>
  page.evaluate((c) => window.__e2e.calls.filter((x) => x.cmd === c).map((x) => x.args), cmd);

test('the core and status line follow the agent state', async ({ page }) => {
  await open(page);
  await expect(page.getByTestId('status-line')).toHaveText(/standby/i);
  await emit(page, 'markon://agent', { type: 'state_changed', data: 'listening' });
  await expect(page.getByRole('img', { name: 'Voice core, listening' })).toBeVisible();
  await expect(page.getByTestId('status-line')).toHaveText(/listening/i);
  await emit(page, 'markon://agent', { type: 'state_changed', data: 'thinking' });
  await expect(page.getByTestId('status-line')).toHaveText(/thinking/i);
});

test('live transcript and reply appear bottom-left with the mic meter', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://audio', { type: 'partial_transcript', text: 'Hey Markon, what is' });
  await expect(page.getByTestId('transcript')).toHaveText(/YOU: Hey Markon, what is/);
  await emit(page, 'markon://audio', {
    type: 'final_transcript',
    text: 'Hey Markon, what is on today?',
    ms: 350,
  });
  await expect(page.getByTestId('transcript')).toHaveText(/what is on today\?/);
  await emit(page, 'markon://agent', { type: 'text_delta', data: { turn_id: 1, text: 'You have ' } });
  await emit(page, 'markon://agent', { type: 'text_delta', data: { turn_id: 1, text: 'three meetings.' } });
  await expect(page.getByTestId('transcript')).toHaveText(/MARKON: You have three meetings\./);
  await emit(page, 'markon://features', {
    type: 'features',
    who: 'user',
    rms: 0.5,
    pitch: 120,
    bands: Array(48).fill(0.3),
  });
  await expect(page.getByTestId('meter')).toHaveAttribute('value', '50');
});

test('typed text is sent to the agent', async ({ page }) => {
  await open(page);
  const box = page.getByRole('textbox', { name: /message markon/i });
  await box.fill('What is the weather?');
  await box.press('Enter');
  expect(await calls(page, 'submit_text')).toEqual([{ text: 'What is the weather?' }]);
  await expect(box).toHaveValue('');
});

test('Esc on an empty canvas hides the window', async ({ page }) => {
  await open(page);
  await page.keyboard.press('Escape');
  expect(await calls(page, 'hide_window')).toHaveLength(1);
});

test('Cmd+Q asks before quitting', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://menu', 'quit');
  const dialog = page.getByRole('alertdialog', { name: /quit markon/i });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  expect(await calls(page, 'quit_app')).toHaveLength(0);
  expect(await calls(page, 'hide_window')).toHaveLength(0);
  await emit(page, 'markon://command', { command: 'quit', arg: '' });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Enter');
  expect(await calls(page, 'quit_app')).toHaveLength(1);
});

test('mic denied shows a banner that opens the privacy setting', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://audio', { type: 'mic_denied' });
  await page.getByRole('button', { name: /open microphone settings/i }).click();
  expect(await calls(page, 'open_mic_settings')).toHaveLength(1);
});

test('no microphone falls back to text with a clear banner', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://audio', { type: 'no_microphone' });
  await expect(page.getByRole('status')).toContainText(/no microphone/i);
  await expect(page.getByRole('textbox', { name: /message markon/i })).toBeFocused();
});

test('missing key and model downloads are explained', async ({ page }) => {
  await open(page, '&has_key=false&models_ready=false&model_note="Downloading kokoro-v1 (40%)"');
  await expect(page.getByRole('status')).toContainText(/anthropic key/i);
  await expect(page.getByRole('status')).toContainText(/Downloading kokoro-v1 \(40%\)/);
});
