import { defineConfig, loadEnv, type Plugin } from 'vite'
import vue from '@vitejs/plugin-vue'
import vueRouter from 'vue-router/vite'
import ui from '@nuxt/ui/vite'
import { fileURLToPath, URL } from 'node:url'
import { resolve } from 'path'
import { readdirSync, readFileSync, writeFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { gzipSync, brotliCompressSync, constants as zlib } from 'node:zlib'

// 构建期为 hashed assets 生成 .gz / .br 兄弟文件（对齐 DSH 2026-10 批次做法）。
// 服务端（app/ui/index.cgi）按 Accept-Encoding 协商发送预压缩字节；
// 只压 assetsDir 下的 js/css（index.html 不压——体积小且改动频繁，<1KB 不值得压）。
function precompressAssets(): Plugin {
  return {
    name: 'qwenpaw-precompress-assets',
    apply: 'build',
    closeBundle() {
      const assetsDir = resolve(__dirname, '../app/www/public')
      let files: string[]
      try {
        files = readdirSync(assetsDir)
      } catch {
        return
      }
      let gz = 0
      let br = 0
      for (const name of files) {
        if (!/\.(js|css)$/.test(name)) continue
        const file = join(assetsDir, name)
        if (statSync(file).size < 1024) continue
        writeFileSync(file + '.gz', gzipSync(readFileSync(file), { level: 9 }))
        gz++
        try {
          writeFileSync(file + '.br', brotliCompressSync(readFileSync(file), {
            params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 }
          }))
          br++
        } catch {
          // brotli 失败不致命，服务端会回退 gzip
        }
      }
      console.log(`[precompress] .gz ×${gz}, .br ×${br}`)
    }
  }
}

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '')
  const basePath = env.VITE_BASE_PATH || '/'

  return {
    plugins: [
      vueRouter({
        dts: 'src/route-map.d.ts'
      }),
      vue(),
      precompressAssets(),
      ui({
        theme: {
          colors: [
            'primary',
            'secondary',
            'success',
            'info',
            'warning',
            'error',
            'neutral',
            'fnui'
          ]
        },
        ui: {
          colors: {
            primary: 'fnui',
            neutral: 'slate'
          }
        }
      })
    ],
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url))
      }
    },
    base: basePath,
    build: {
      outDir: resolve(__dirname, '../app/www'),
      emptyOutDir: true,
      assetsDir: 'public',
      rollupOptions: {
        output: {
          manualChunks: undefined,
        },
      },
    },
  }
})
