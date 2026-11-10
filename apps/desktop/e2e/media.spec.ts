import { expect, type Page, test } from '@playwright/test';

/* pictures, audio, video and Spotify shown on Markon's own screen. */
const open = async (page: Page, responses: Record<string, unknown> = {}) => {
  await page.addInitScript((r) => {
    (window as unknown as { __e2eResponses: unknown }).__e2eResponses = r;
  }, responses);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/?e2e');
  await page.waitForFunction(() => window.__e2e !== undefined);
};
const emit = (page: Page, event: string, payload: unknown) =>
  page.evaluate(([e, p]) => window.__e2e.emit(e as string, p), [event, payload] as const);
const calls = (page: Page, cmd: string) =>
  page.evaluate((c) => window.__e2e.calls.filter((x) => x.cmd === c).map((x) => x.args), cmd);
const card = (id: string, kind: string, title: string, body: Record<string, unknown>) => ({
  id,
  turn_id: 1,
  kind,
  title,
  status: 'done',
  body: { kind, title, ...body },
  updated_ms: Date.now(),
});

test('a picture card shows the image with its credit, at double height', async ({ page }) => {
  await open(page);
  await emit(
    page,
    'markon://card',
    card('i', 'image', 'Cat in a box', {
      path: '/Users/me/.markon/cache/images/abc.jpg',
      source: 'https://commons.wikimedia.org/wiki/File:Cat.jpg',
      credit: 'Ana · CC BY-SA 4.0',
    }),
  );
  const c = page.getByRole('article', { name: /picture: cat in a box/i });
  await expect(c).toBeVisible();
  const img = c.getByRole('img', { name: 'Cat in a box' });
  await expect(img).toHaveAttribute('src', /abc\.jpg/);
  await expect(c).toContainText('Ana · CC BY-SA 4.0');
  await expect(c).toHaveAttribute('data-big', 'true');
});

test('a YouTube card embeds the privacy-enhanced player', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://card', card('v', 'video', 'Funny cats', { youtube: 'dQw4w9WgXcQ' }));
  const frame = page.getByTitle('Funny cats');
  await expect(frame).toHaveAttribute('src', /^https:\/\/www\.youtube-nocookie\.com\/embed\/dQw4w9WgXcQ\?/);
});

test('local audio and video play from their file, and duck while Markon talks', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://card', card('a', 'audio', 'Song', { path: '/Users/me/Music/song.mp3' }));
  await emit(page, 'markon://card', card('m', 'video', 'Clip', { path: '/Users/me/Movies/clip.mp4' }));
  const audio = page.locator('audio');
  await expect(audio).toHaveAttribute('src', /song\.mp3/);
  await expect(page.locator('video')).toHaveAttribute('src', /clip\.mp4/);
  expect(await audio.evaluate((a: HTMLAudioElement) => a.volume)).toBe(1);
  await emit(page, 'markon://agent', { type: 'state_changed', data: 'speaking' });
  await expect.poll(() => audio.evaluate((a: HTMLAudioElement) => a.volume)).toBe(0.2);
  await emit(page, 'markon://agent', { type: 'state_changed', data: 'idle' });
  await expect.poll(() => audio.evaluate((a: HTMLAudioElement) => a.volume)).toBe(1);
});

test('the Spotify card shows the track and its buttons control Spotify', async ({ page }) => {
  const playing = {
    kind: 'music',
    track: 'Song',
    artist: 'Band',
    album: 'Album',
    artwork: '',
    state: 'playing',
    position_s: 30,
    duration_s: 200,
  };
  await open(page, { spotify_control: playing });
  await emit(page, 'markon://card', card('s', 'music', 'Song', playing));
  const c = page.getByRole('article', { name: /music: song/i });
  await expect(c).toContainText('Band');
  await c.getByRole('button', { name: 'Pause' }).click();
  await c.getByRole('button', { name: 'Next track' }).click();
  const actions = (await calls(page, 'spotify_control')).map((a) => (a as { action: string }).action);
  expect(actions).toEqual(expect.arrayContaining(['pause', 'next']));
});

test('a YouTube video fills its card', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://card', card('v', 'video', 'Funny cats', { youtube: 'dQw4w9WgXcQ' }));
  const body = page.getByRole('article', { name: /video: funny cats/i }).getByTestId('card-body');
  const frame = page.getByTitle('Funny cats');
  await expect(frame).toBeVisible();
  const [b, f] = await Promise.all([body.boundingBox(), frame.boundingBox()]);
  expect(f?.height).toBeGreaterThan((b?.height ?? 0) - 4);
});

test('each card has a working close button', async ({ page }) => {
  await open(page);
  await emit(page, 'markon://card', card('v', 'video', 'Funny cats', { youtube: 'dQw4w9WgXcQ' }));
  await emit(page, 'markon://card', card('i', 'image', 'Cat', { path: '/x/cat.jpg' }));
  await page.getByRole('button', { name: 'Close Funny cats' }).click();
  await expect(page.getByTitle('Funny cats')).toHaveCount(0);
  await expect(page.getByRole('article', { name: /picture: cat/i })).toBeVisible();
});

test('Markon closes cards when asked: by title, media, latest or all', async ({ page }) => {
  await open(page);
  const close = (which: string) =>
    emit(page, 'markon://card', { ...card(`c-${which}`, 'note', 'close', { which }), status: 'closed' });
  await emit(page, 'markon://card', card('n', 'note', 'Shopping list', { text: 'milk' }));
  await emit(page, 'markon://card', card('v', 'video', 'Summertime Sadness', { youtube: 'dQw4w9WgXcQ' }));
  await emit(page, 'markon://card', card('w', 'video', 'Summertime Sadness', { youtube: 'dQw4w9WgXcQ' }));
  await emit(page, 'markon://card', card('i', 'image', 'Cat', { path: '/x/cat.jpg' }));
  await close('summertime');
  await expect(page.getByTitle('Summertime Sadness')).toHaveCount(0);
  await expect(page.getByRole('article')).toHaveCount(2);
  await close('media');
  await expect(page.getByRole('article', { name: /picture: cat/i })).toHaveCount(0);
  await emit(page, 'markon://card', card('n2', 'note', 'Second note', { text: 'x' }));
  await close('latest');
  await expect(page.getByRole('article', { name: /second note/i })).toHaveCount(0);
  await expect(page.getByRole('article')).toHaveCount(1);
  await close('all');
  await expect(page.getByRole('article')).toHaveCount(0);
});
