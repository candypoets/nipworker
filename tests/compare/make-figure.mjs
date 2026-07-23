import { chromium } from '@playwright/test';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const figureSource = resolve(scriptDirectory, 'figure.html');
const outputPath = resolve(scriptDirectory, '..', '..', 'benchmark-figure.png');

const browser = await chromium.launch();
const page = await browser.newPage({
	viewport: { width: 1400, height: 1300 },
	deviceScaleFactor: 2
});
await page.goto(pathToFileURL(figureSource).href);
await page.waitForTimeout(400);
const box = await page.locator('body').boundingBox();
await page.screenshot({
	path: outputPath,
	clip: { x: 0, y: 0, width: 1400, height: Math.ceil(box.height) }
});
await browser.close();
console.log(`written: ${outputPath}`);
