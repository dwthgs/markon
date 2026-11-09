import { defineConfig } from '@playwright/test';

// The UI runs in a browser against the Vite server with the Tauri side faked.
export default defineConfig({
  testDir: 'e2e',
  timeout: 30_000,
  use: { baseURL: 'http://localhost:5173', viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 },
  webServer: {
    command: 'pnpm vite --port 5173 --strictPort',
    url: 'http://localhost:5173',
    reuseExistingServer: true,
  },
});
