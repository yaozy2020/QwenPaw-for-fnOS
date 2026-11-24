import { build } from 'vite';
import vue from '@vitejs/plugin-vue';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const root = fileURLToPath(new URL('.', import.meta.url));
export async function buildPanel(buildId = 'dev', { testHooks = false } = {}) {
  const manifest = await readFile(join(root, '../manifest'), 'utf8');
  const appVersion = manifest.match(/^\s*version\s*=\s*(\S+)\s*$/m)?.[1] ?? '';
  if (!appVersion) throw new Error('Unable to extract version from manifest');
  const { output } = await build({
    root, configFile: false, envDir: false, publicDir: false, logLevel: 'warn', plugins: [vue()],
    define: {
      __APP_VERSION__: JSON.stringify(appVersion),
      __PANEL_TEST__: testHooks,
      __VUE_OPTIONS_API__: false,
      __VUE_PROD_DEVTOOLS__: false,
      __VUE_PROD_HYDRATION_MISMATCH_DETAILS__: false,
    },
    build: {
      target: ['chrome111', 'edge111', 'firefox114', 'safari16.4'], write: false, cssCodeSplit: false, modulePreload: false,
      rolldownOptions: { input: join(root, 'src/main.ts'), output: { format: 'iife', entryFileNames: 'main.js', assetFileNames: 'main.[ext]' } },
    },
  });
  const script = output.find(file => file.fileName === 'main.js');
  const sheet = output.find(file => file.fileName === 'main.css');
  if (output.length !== 2 || script?.type !== 'chunk' || sheet?.type !== 'asset') throw new Error('Expected a self-contained JS and CSS pair');
  const js = script.code, css = String(sheet.source);
  const safeJs = js.replace(/<\/script>/gi, '<\\/script>').replace(/<!--/g, '\\x3C!--');
  const template = await readFile(join(root, 'src/index.html'), 'utf8');
  const html = template.replace('/*__INLINE_CSS__*/', () => css.replace(/<\/style>/gi, '<\\/style>'))
    .replace('//__INLINE_JS__', () => safeJs).replace('__BUILD_ID__', () => buildId);
  return { html, js, css };
}
