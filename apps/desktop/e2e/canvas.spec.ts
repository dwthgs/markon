import { expect, type Page, test } from '@playwright/test';

/* With the Rust side faked. */
const open = async (page: Page, size = { width: 1440, height: 900 }) => {
  await page.setViewportSize(size);
  await page.goto('/?e2e');
  await page.waitForFunction(() => window.__e2e !== undefined);
  await expect(page.getByTestId('status-line')).toBeVisible();
};
const emit = (page: Page, event: string, payload: unknown) =>
  page.evaluate(([e, p]) => window.__e2e.emit(e as string, p), [event, payload] as const);
const calls = (page: Page, cmd: string) =>
  page.evaluate((c) => window.__e2e.calls.filter((x) => x.cmd === c).map((x) => x.args), cmd);
const card = (id: string, kind: string, title: string, extra: Record<string, unknown> = {}) => ({
  id,
  turn_id: 1,
  kind,
  title,
  status: 'done',
  body: { preview: `${title} body` },
  updated_ms: Date.now(),
  ...extra,
});
const coreCentre = async (page: Page) => {
  const b = await page.getByRole('img', { name: /voice core/i }).boundingBox();
  if (!b) throw new Error('no core');
  return { x: b.x + b.width / 2, y: b.y + b.height / 2, w: b.width };
};

test('cards flow around the core, which stays centred and only shrinks a little', async ({ page }) => {
  await open(page);
  const before = await coreCentre(page);
  await emit(
    page,
    'markon://card',
    card('a', 'agent', 'State of VR', {
      status: 'running',
      body: {
        steps: [
          { name: 'fetch_page', status: 'done' },
          { name: 'read_file', status: 'running' },
        ],
        started_ms: Date.now() - 5000,
      },
    }),
  );
  await emit(page, 'markon://card', card('f', 'file', '/Users/me/notes.txt'));
  await emit(
    page,
    'markon://card',
    card('p', 'page', 'https://example.com', { body: { title: 'Example', preview: 'Facts' } }),
  );
  await emit(page, 'markon://card', card('r', 'result', 'Report', { body: { text: 'Forty-two blocks.' } }));
  await expect(page.getByRole('article')).toHaveCount(4);
  const after = await coreCentre(page);
  expect(Math.abs(after.x - before.x)).toBeLessThanOrEqual(1);
  expect(Math.abs(after.y - before.y)).toBeLessThanOrEqual(1);
  expect(after.w).toBeLessThan(before.w);
  expect(after.w).toBeGreaterThan(before.w * 0.75);
  // No card overlaps the core's circle.
  for (const a of await page.getByRole('article').all()) {
    const b = await a.boundingBox();
    if (!b) continue;
    const cx = Math.max(b.x, Math.min(after.x, b.x + b.width));
    const cy = Math.max(b.y, Math.min(after.y, b.y + b.height));
    expect(Math.hypot(cx - after.x, cy - after.y)).toBeGreaterThan(after.w * 0.33);
  }
  await expect(page.getByRole('article', { name: /agent: state of vr/i })).toContainText('1/2');
});

test('cards are focusable, text is selectable, long results scroll inside', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://card', card('r', 'result', 'Long', { body: { text: 'line\n'.repeat(400) } }));
  const a = page.getByRole('article', { name: /result: long/i });
  await a.focus();
  await expect(a).toBeFocused();
  const scroll = a.getByTestId('card-body');
  expect(
    await scroll.evaluate((el) => el.scrollHeight > el.clientHeight && getComputedStyle(el).overflowY),
  ).toBe('auto');
  expect(await scroll.evaluate((el) => getComputedStyle(el).userSelect)).not.toBe('none');
});

test('an update highlights the card instead of moving it', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://card', card('f', 'file', 'a.txt'));
  await emit(page, 'markon://card', card('g', 'file', 'b.txt'));
  const a = page.getByRole('article', { name: /file: a\.txt/i });
  await page.waitForTimeout(600); // let the entry animation finish
  const b1 = await a.boundingBox();
  await emit(
    page,
    'markon://card',
    card('f', 'file', 'a.txt', { updated_ms: Date.now() + 10, body: { preview: 'changed' } }),
  );
  await expect(a).toHaveAttribute('data-updated', 'true');
  expect(await a.boundingBox()).toEqual(b1);
});

test('Esc clears the canvas first, then hides the window', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://card', card('f', 'file', 'a.txt'));
  await expect(page.getByRole('article')).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('article')).toHaveCount(0);
  expect(await calls(page, 'hide_window')).toHaveLength(0);
  await page.keyboard.press('Escape');
  expect(await calls(page, 'hide_window')).toHaveLength(1);
});

test('controls and voice commands: clear view, tidy, focus on', async ({ page }) => {
  await open(page);
  const titles: [string, string][] = [
    ['1', 'Market snapshot'],
    ['2', 'Shipments'],
    ['3', 'Key players'],
  ];
  for (const [id, t] of titles)
    await emit(page, 'markon://card', card(id, 'result', t, { body: { text: t } }));
  await expect(page.getByRole('article')).toHaveCount(3);
  await emit(page, 'markon://command', { command: 'focus', arg: 'shipments' });
  await expect(page.getByRole('article', { name: /shipments/i })).toBeFocused();
  await expect(page.getByRole('article', { name: /shipments/i })).toHaveAttribute('data-big', 'true');
  await page.getByRole('button', { name: 'Tidy' }).click();
  await expect(page.getByRole('article')).toHaveCount(3);
  await page.getByRole('button', { name: 'Clear view' }).click();
  await expect(page.getByRole('article')).toHaveCount(0);
  for (const name of ['History', 'Tidy', 'Clear view', 'Camera'])
    await expect(page.getByRole('button', { name })).toBeVisible();
});

test('more cards than fit overflow into a row with a count badge', async ({ page }) => {
  await open(page);
  for (let i = 0; i < 30; i++)
    await emit(page, 'markon://card', card(`c${i}`, 'note', `Note ${i}`, { body: { text: `n${i}` } }));
  await expect(page.getByTestId('overflow-badge')).toBeVisible();
  const shown = await page.getByTestId('canvas-zone').getByRole('article').count();
  await expect(page.getByTestId('overflow-badge')).toHaveText(`+${30 - shown}`);
});

test('below 1024 px wide the cards collapse to one column', async ({ page }) => {
  await open(page, { width: 900, height: 800 });
  for (const id of ['a', 'b', 'c'])
    await emit(page, 'markon://card', card(id, 'note', `N ${id}`, { body: { text: id } }));
  const xs = new Set<number>();
  for (const a of await page.getByRole('article').all()) xs.add(Math.round((await a.boundingBox())?.x ?? 0));
  expect(xs.size).toBe(1);
  const c = await coreCentre(page);
  expect(Math.abs(c.x - 450)).toBeLessThanOrEqual(1);
});

test('a permission request shows the exact command and sends the answer', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://permission', {
    id: 7,
    tier: 'execute',
    tool: 'run_command',
    detail: 'ls -la ~/Documents',
  });
  const d = page.getByRole('alertdialog', { name: /allow markon to run/i });
  await expect(d).toContainText('ls -la ~/Documents');
  await d.getByRole('button', { name: 'Deny' }).click();
  expect(await calls(page, 'answer_permission')).toEqual([{ id: 7, decision: 'deny' }]);
  await emit(page, 'markon://permission', {
    id: 8,
    tier: 'write',
    tool: 'write_file',
    detail: '/Users/me/Notes',
  });
  await page.getByRole('alertdialog').getByRole('button', { name: 'Allow' }).click();
  expect((await calls(page, 'answer_permission'))[1]).toEqual({ id: 8, decision: 'allow' });
});

test('a cancelled turn removes its cards', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://card', card('x', 'file', 'x.txt', { turn_id: 5 }));
  await emit(page, 'markon://card', card('y', 'file', 'y.txt', { turn_id: 6 }));
  await emit(page, 'markon://agent', { type: 'turn_cancelled', data: { turn_id: 5 } });
  await expect(page.getByRole('article')).toHaveCount(1);
});

test('24 cards keep the frame rate up', async ({ page }) => {
  await open(page);
  for (let i = 0; i < 24; i++)
    await emit(
      page,
      'markon://card',
      card(`k${i}`, i % 2 ? 'page' : 'result', `Card ${i}`, {
        body: { text: 'x '.repeat(200), preview: 'y '.repeat(200) },
      }),
    );
  const fps = await page.evaluate(
    () =>
      new Promise<number>((done) => {
        let n = 0;
        const t0 = performance.now();
        const tick = (t: number) => {
          n++;
          if (t - t0 >= 2000) done((n * 1000) / (t - t0));
          else requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
      }),
  );
  console.log(`24 cards: ${fps.toFixed(1)} fps (headless Chromium)`);
  expect(fps).toBeGreaterThanOrEqual(50);
});

test('while thinking, a Stop button and Esc cancel the reply', async ({ page }) => {
  await open(page);
  await expect(page.getByRole('button', { name: /stop/i })).toBeHidden();
  await emit(page, 'markon://agent', { type: 'state_changed', data: 'thinking' });
  await page.getByRole('button', { name: /stop/i }).click();
  expect(await calls(page, 'cancel')).toHaveLength(1);
  await emit(page, 'markon://agent', { type: 'state_changed', data: 'speaking' });
  await page.keyboard.press('Escape');
  expect(await calls(page, 'cancel')).toHaveLength(2);
  expect(await calls(page, 'hide_window')).toHaveLength(0);
});
