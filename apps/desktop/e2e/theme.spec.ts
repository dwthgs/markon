import { expect, test } from '@playwright/test';

/* "Activate Eclipse" swaps to the GridCN Ares theme at once; "normal mode" swaps back. */
test('the theme command switches between Tron and Ares', async ({ page }) => {
  await page.goto('/?e2e');
  await page.waitForFunction(() => window.__e2e !== undefined);
  const primary = () =>
    page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--primary').trim());
  const say = (arg: string) =>
    page.evaluate((a) => window.__e2e.emit('markon://command', { command: 'theme', arg: a }), arg);

  expect(await primary()).toBe('oklch(0.75 0.18 195)');
  await say('ares');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'ares');
  expect(await primary()).toBe('oklch(0.6 0.25 25)');
  await say('tron');
  expect(await primary()).toBe('oklch(0.75 0.18 195)');
});
