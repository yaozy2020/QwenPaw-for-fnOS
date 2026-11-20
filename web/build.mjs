import { mkdir, writeFile, copyFile } from 'node:fs/promises';
import { buildPanel } from './build-panel.mjs';

const buildId = process.env.BUILD_ID || 'dev';
const { html, js, css } = await buildPanel(buildId);
await mkdir('dist', { recursive: true });
await Promise.all([
  writeFile('dist/index.html', html), writeFile('dist/main.js', js), writeFile('dist/main.css', css),
  copyFile(new URL('./src/auth-callback.html', import.meta.url), 'dist/auth-callback.html'),
]);
console.log(`dist/index.html: ${(Buffer.byteLength(html) / 1024).toFixed(1)} KiB (build ${buildId})`);
