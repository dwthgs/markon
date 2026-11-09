import { expect, test } from '@playwright/test';

/* History reloads a conversation's cards onto the canvas. */
test('History lists conversations and reloads their cards', async ({ page }) => {
  await page.goto('/?e2e');
  await page.waitForFunction(() => window.__e2e !== undefined);
  await page.evaluate(() => {
    window.__e2e.responses.history_list = [
      { id: 2, started_at: Date.now(), title: 'Morning briefing', source: 'schedule' },
      { id: 1, started_at: Date.now() - 86_400_000, title: 'State of VR', source: 'voice' },
    ];
  });
  await page.getByRole('button', { name: 'History' }).click();
  const panel = page.getByRole('dialog', { name: /history/i });
  await expect(panel.getByRole('button')).toHaveCount(2);
  await panel.getByRole('button', { name: /state of vr/i }).click();
  const calls = await page.evaluate(() =>
    window.__e2e.calls.filter((c) => c.cmd === 'open_conversation').map((c) => c.args),
  );
  expect(calls).toEqual([{ id: 1 }]);
  await expect(panel).toBeHidden();
  await page.evaluate(() =>
    window.__e2e.emit('markon://cards-load', [
      {
        id: 'result-1',
        turn_id: 1,
        kind: 'result',
        title: 'State of VR',
        status: 'done',
        body: { text: '42 blocks' },
        updated_ms: 1,
      },
    ]),
  );
  await expect(page.getByRole('article', { name: /result: state of vr/i })).toContainText('42 blocks');
});

test('Esc closes History without hiding the window', async ({ page }) => {
  await page.goto('/?e2e');
  await page.waitForFunction(() => window.__e2e !== undefined);
  await page.evaluate(() => {
    window.__e2e.responses.history_list = [];
  });
  await page.getByRole('button', { name: 'History' }).click();
  await expect(page.getByRole('dialog', { name: /history/i })).toContainText(/nothing yet/i);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: /history/i })).toBeHidden();
  expect(await page.evaluate(() => window.__e2e.calls.filter((c) => c.cmd === 'hide_window').length)).toBe(0);
});
