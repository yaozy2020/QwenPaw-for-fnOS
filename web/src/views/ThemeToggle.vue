<script setup lang="ts">
import { computed, onMounted, onUnmounted, watchEffect } from 'vue';
import { t, type MessageKey } from '../i18n';
import {
  currentTheme,
  LEGACY_THEME_STORAGE_KEY,
  syncDocumentTheme,
  syncThemeFromStorage,
  syncThemeFromSystem,
  syncUiStyleFromStorage,
  THEME_STORAGE_KEY,
  themeMode,
  toggleTheme,
  UI_STYLE_STORAGE_KEY,
  uiStyle,
} from '../theme';

const media = window.matchMedia?.('(prefers-color-scheme: dark)');

const labelKey = computed<MessageKey>(() =>
  currentTheme.value === 'dark' ? 'theme.aria.darkToLight' : 'theme.aria.lightToDark',
);
const label = computed(() => t(labelKey.value));
const modeText = computed(() => currentTheme.value === 'dark' ? t('theme.dark') : t('theme.light'));

watchEffect(() => {
  syncDocumentTheme(currentTheme.value, themeMode.value, uiStyle.value);
});

function toggle() {
  toggleTheme(true);
}
function systemChanged(event: MediaQueryListEvent) { syncThemeFromSystem(event.matches); }
function storageChanged(event: StorageEvent) {
  if (event.key !== null && event.key !== THEME_STORAGE_KEY && event.key !== UI_STYLE_STORAGE_KEY) return;
  try {
    if (event.storageArea !== localStorage) return;
    if (event.key === null || event.key === THEME_STORAGE_KEY) syncThemeFromStorage();
    if (event.key === null || event.key === UI_STYLE_STORAGE_KEY) syncUiStyleFromStorage();
  } catch { /* Storage is unavailable. */ }
}
onMounted(() => {
  try { localStorage.removeItem(LEGACY_THEME_STORAGE_KEY); } catch { /* Obsolete preference. */ }
  if (media) syncThemeFromSystem(!!media.matches);
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
  <button type="button" class="btn-outline" id="themeToggleBtn" :data-theme="currentTheme" :data-theme-mode="themeMode" @click="toggle" :title="label" :aria-label="label">
    <svg v-if="currentTheme === 'light'" id="themeIconSun" viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <circle cx="12" cy="12" r="5"/>
      <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/>
    </svg>
    <svg v-else id="themeIconMoon" viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <path d="M21 12.79A9 9 0 1111.21 3 7 7 0 0021 12.79z"/>
    </svg>
    <span class="btn-label" id="themeText">{{ modeText }}</span>
  </button>
</template>
