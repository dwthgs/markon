import { expect, test } from '@playwright/test';

test.describe('shell', () => {
  test('dark full-bleed stage with Core Prime centred', async ({ page }) => {
    await page.goto('/');
    const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(bg).toBe('rgb(3, 6, 12)');
    const core = page.getByRole('img', { name: /voice core/i });
    await expect(core).toBeVisible();
    const box = await core.boundingBox();
    if (!box) throw new Error('core has no box');
    const vp = page.viewportSize();
    if (!vp) throw new Error('no viewport');
    expect(Math.abs(box.x + box.width / 2 - vp.width / 2)).toBeLessThanOrEqual(1);
    expect(Math.abs(box.y + box.height / 2 - vp.height / 2)).toBeLessThanOrEqual(1);
  });

  test('a drag strip spans the top and does not cover the controls', async ({ page }) => {
    await page.goto('/');
    const strip = page.getByTestId('drag-strip');
    await expect(strip).toHaveAttribute('data-tauri-drag-region');
    const box = await strip.boundingBox();
    if (!box) throw new Error('strip has no box');
    expect(box.y).toBe(0);
    expect(box.width).toBe(page.viewportSize()?.width);
    // The History button sits inside the strip's band and must still receive the click.
    const history = page.getByRole('button', { name: 'History' });
    const hb = await history.boundingBox();
    if (!hb) throw new Error('no history button');
    const hit = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.textContent, {
      x: hb.x + hb.width / 2,
      y: hb.y + hb.height / 2,
    });
    expect(hit).toBe('History');
  });

  test('the core animates', async ({ page }) => {
    await page.goto('/');
    const core = page.getByRole('img', { name: /voice core/i });
    const a = await core.screenshot();
    await page.waitForTimeout(300);
    const b = await core.screenshot();
    expect(a.equals(b)).toBe(false);
  });

  test('makes no network request outside the app (fonts are local)', async ({ page }) => {
    const external: string[] = [];
    page.on('request', (r) => {
      if (!r.url().startsWith('http://localhost:5173')) external.push(r.url());
    });
    await page.goto('/');
    await page.waitForLoadState('networkidle');
    expect(external).toEqual([]);
  });
});
