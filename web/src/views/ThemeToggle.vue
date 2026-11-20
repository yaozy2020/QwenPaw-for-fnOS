<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watchEffect } from 'vue';

type Theme = 'light' | 'dark';
const key = 'qwenpaw_theme_v2';
function readChoice(): Theme | null {
  try {
    const saved = localStorage.getItem(key);
    return saved === 'light' || saved === 'dark' ? saved : null;
  } catch { return null; }
}
const choice = ref(readChoice());
const media = window.matchMedia?.('(prefers-color-scheme: dark)');
const systemDark = ref(!!media?.matches);
const theme = computed(() => choice.value ?? (systemDark.value ? 'dark' : 'light'));
const label = computed(() => theme.value === 'light' ? '白天模式，切换到暗夜模式' : '暗夜模式，切换到白天模式');
watchEffect(() => { document.documentElement.dataset.theme = theme.value; });

function toggle() {
  choice.value = theme.value === 'light' ? 'dark' : 'light';
  try { localStorage.setItem(key, choice.value); } catch { /* Keep the choice for this page. */ }
}
function systemChanged(event: MediaQueryListEvent) { systemDark.value = event.matches; }
function storageChanged(event: StorageEvent) {
  if (event.key !== null && event.key !== key) return;
  try { if (event.storageArea === localStorage) choice.value = readChoice(); } catch { /* Storage is unavailable. */ }
}
onMounted(() => {
  try { localStorage.removeItem('qwenpaw_theme'); } catch { /* Obsolete preference. */ }
  if (media?.addEventListener) media.addEventListener('change', systemChanged);
  else media?.addListener?.(systemChanged);
  window.addEventListener('storage', storageChanged);
});
onUnmounted(() => {
  if (media?.removeEventListener) media.removeEventListener('change', systemChanged);
  else media?.removeListener?.(systemChanged);
  window.removeEventListener('storage', storageChanged);
});
</script>

<template>
  <button type="button" class="btn-outline" id="themeToggleBtn" @click="toggle" :title="label" :aria-label="label">
    <svg v-if="theme === 'light'" id="themeIconSun" viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <circle cx="12" cy="12" r="5"/>
      <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/>
    </svg>
    <svg v-else id="themeIconMoon" viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <path d="M21 12.79A9 9 0 1111.21 3 7 7 0 0021 12.79z"/>
    </svg>
    <span class="btn-label" id="themeText">{{ theme === 'light' ? '白天' : '暗夜' }}</span>
  </button>
</template>
