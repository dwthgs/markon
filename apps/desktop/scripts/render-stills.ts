// Renders the app icon source and the per-state tray images from the Minimal core.
// Run: pnpm --filter @markon/desktop stills
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';
import { createServer } from 'vite';

const root = fileURLToPath(new URL('../../../packages/voice-core/harness', import.meta.url));
const icons = fileURLToPath(new URL('../src-tauri/icons/', import.meta.url));
const server = await createServer({ root, server: { port: 5175, strictPort: true }, logLevel: 'error' });
await server.listen();
const browser = await chromium.launch();
const page = await browser.newPage();
await page.goto('http://localhost:5175/');
await page.waitForFunction(() => 'harness' in window);
const save = (file: string, url: string) =>
  writeFileSync(icons + file, Buffer.from(url.split(',')[1] as string, 'base64'));
save('source.png', await page.evaluate(() => window.harness.still('minimal', 'speaking', 1024, 90, true)));
for (const state of ['idle', 'listening', 'thinking', 'speaking'] as const) {
  save(
    `tray-${state}.png`,
    await page.evaluate((s) => window.harness.still('minimal', s, 64, 150, false), state),
  );
}
await browser.close();
await server.close();
