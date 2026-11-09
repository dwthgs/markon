import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: 'e2e',
  timeout: 60_000,
  use: { viewport: { width: 1280, height: 900 }, deviceScaleFactor: 1 },
  webServer: {
    command: 'pnpm vite harness --port 5174 --strictPort',
    url: 'http://localhost:5174',
    reuseExistingServer: true,
  },
});
