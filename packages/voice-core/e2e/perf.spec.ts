import { expect, test } from '@playwright/test';

// Under 4 ms per frame at 800 px.
test('Core Prime draws an 800 px frame in under 4 ms', async ({ page }) => {
  await page.goto('http://localhost:5174/');
  await page.waitForFunction(() => 'harness' in window);
  const times = await page.evaluate(() => {
    const h = window.harness;
    h.setup(3, 'prime', 800, true);
    h.setState('speaking');
    h.timeFrames(120); // warm up the JIT
    return h.timeFrames(600);
  });
  times.sort((a, b) => a - b);
  const median = times[Math.floor(times.length / 2)] as number;
  const p95 = times[Math.floor(times.length * 0.95)] as number;
  console.log(`prime 800px: median ${median.toFixed(2)} ms, p95 ${p95.toFixed(2)} ms`);
  expect(median).toBeLessThan(4);
});
