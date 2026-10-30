import './assets/css/main.css'

import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import { routes, handleHotUpdate } from 'vue-router/auto-routes'
import ui from '@nuxt/ui/vue-plugin'

import App from './App.vue'

const app = createApp(App)

const basePath = import.meta.env.BASE_URL

const router = createRouter({
  routes,
  history: createWebHistory(basePath)
})

// chunk 加载兜底（对齐 DSH 2026-10 实践）：升级后旧 index.html 引用失效 hash、
// 或网络抖动导致动态 import 失败时，整页重载一次拿最新产物。
// 陷阱：不能无条件 preventDefault——被节流拦下还吞掉错误的话，Vite 会让动态 import
// 以 undefined 解析、defineAsyncComponent 缓存成「已解析」后永不重试（永久空白）。
// 节流命中改弹原生 DOM 刷新横幅；CSS 预加载失败不重载（避免丢页面状态）。
const RELOAD_THROTTLE_MS = 30_000
let lastChunkReload = 0
window.addEventListener('vite:preloadError', (e) => {
  e.preventDefault()
  const payload = (e as unknown as { payload?: unknown }).payload
  const target = payload instanceof Event ? (payload.target as HTMLLinkElement | undefined) : undefined
  const isCss = !!target && (target.as === 'style' || (target.rel ?? '').includes('stylesheet'))
  if (isCss) return
  const now = Date.now()
  if (now - lastChunkReload > RELOAD_THROTTLE_MS) {
    lastChunkReload = now
    window.location.reload()
    return
  }
  if (!document.getElementById('chunk-reload-banner')) {
    const banner = document.createElement('div')
    banner.id = 'chunk-reload-banner'
    banner.textContent = '资源已更新，请刷新页面'
    banner.setAttribute('style', 'position:fixed;top:0;left:0;right:0;z-index:2147483647;padding:8px 16px;text-align:center;background:#1e293b;color:#fff;font:14px/1.5 sans-serif;cursor:pointer')
    banner.onclick = () => window.location.reload()
    document.body.appendChild(banner)
  }
})

app.use(router)
app.use(ui)

app.mount('#app')

// favicon 跟随运行时 base（index.html 里的绝对路径 /favicon.png 在 cgi 子路径下会 404）
{
  const fav = document.querySelector<HTMLLinkElement>('link[rel="icon"]')
  if (fav) fav.href = new URL('favicon.png', document.baseURI).href
}

if (import.meta.hot) {
  handleHotUpdate(router)
}
