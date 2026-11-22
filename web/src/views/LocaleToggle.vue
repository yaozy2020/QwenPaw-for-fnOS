<script setup lang="ts">
import { computed, onMounted, onUnmounted, watchEffect } from 'vue';
import {
  currentLocale,
  LOCALE_STORAGE_KEY,
  localeMode,
  syncDocumentLocale,
  syncLocaleFromStorage,
  syncLocaleFromSystem,
  t,
  toggleLocale,
} from '../i18n';

const ariaLabel = computed(() => t('locale.switchAria'));
const text = computed(() => t('locale.label'));
const shortText = computed(() => t('locale.short'));

watchEffect(() => {
  syncDocumentLocale(currentLocale.value, localeMode.value);
});

function storageChanged(event: StorageEvent) {
  if (event.key !== null && event.key !== LOCALE_STORAGE_KEY) return;
  try {
    if (event.storageArea === localStorage) syncLocaleFromStorage();
  } catch { /* Storage is unavailable. */ }
}

onMounted(() => {
  window.addEventListener('storage', storageChanged);
  window.addEventListener('languagechange', syncLocaleFromSystem);
});
onUnmounted(() => {
  window.removeEventListener('storage', storageChanged);
  window.removeEventListener('languagechange', syncLocaleFromSystem);
});
</script>

<template>
  <button type="button" class="btn-outline" id="localeToggleBtn" :data-locale="currentLocale" :data-locale-mode="localeMode" @click="toggleLocale()" :title="ariaLabel" :aria-label="ariaLabel">
    <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <circle cx="12" cy="12" r="9"/>
      <path d="M3.6 9h16.8M3.6 15h16.8M12 3a14 14 0 010 18M12 3a14 14 0 000 18"/>
    </svg>
    <span class="btn-label" id="localeText">{{ text }}</span>
    <span class="locale-short" aria-hidden="true">{{ shortText }}</span>
  </button>
</template>
