import { chromium } from '@playwright/test';

const browser = await chromium.launch();
const page = await browser.newPage();
page.setDefaultTimeout(60000);
const logs = [];
page.on('console', msg => {
  const text = `[${msg.type()}] ${msg.text()}`;
  logs.push(text);
});
page.on('pageerror', err => {
  console.log('[PAGE ERROR]', err.message);
});
page.on('worker', worker => {
  worker.on('console', msg => {
    const text = `[worker:${msg.type()}] ${msg.text()}`;
    logs.push(text);
  });
});

await page.goto('http://localhost:5173/tests/e2e-browser/index.html');
await page.waitForFunction(() => window.__testResults !== null, { timeout: 55000 });
const results = await page.evaluate(() => window.__testResults);

// Print only storage/cache related logs
console.log('\n=== FILTERED LOGS ===');
logs.filter(l => 
  l.includes('IndexedDbStorage') || 
  l.includes('CacheWorker') || 
  l.includes('persist') || 
  l.includes('query') ||
  l.includes('save_to_db') ||
  l.includes('cache') && l.includes('event')
).forEach(l => console.log(l));

console.log('\n=== RESULTS ===');
console.log(JSON.stringify(results, null, 2));
await browser.close();
