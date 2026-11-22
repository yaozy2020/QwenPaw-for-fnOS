<script setup lang="ts">
import PawIcon from './PawIcon.vue';
import { sectionLabel, sectionOrder } from '../navigation';
import { usePanel } from '../panelContext';
import { t } from '../i18n';
const { navigation } = usePanel();
</script>

<template>
  <nav id="mobileSectionNav" class="section-nav" :aria-label="t('nav.ariaLabel')">
    <div class="rail-brand">
      <span class="rail-brand-icon" aria-hidden="true">
        <PawIcon :size="28" />
      </span>
      <span class="rail-brand-name">QwenPaw</span>
    </div>
    <a v-for="id in sectionOrder" :key="id" :href="`#${id}`" :data-section="id" :aria-current="navigation.section === id ? 'page' : undefined" @click="navigation.navigate(id, $event)">
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <template v-if="id === 'overview'"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/></template>
        <template v-else-if="id === 'configuration'"><path d="M4 7h5m4 0h7M4 17h10m4 0h2"/><circle cx="11" cy="7" r="2"/><circle cx="16" cy="17" r="2"/></template>
        <template v-else-if="id === 'authorization'"><path d="m12 3 8 3v6c0 4-4 7.5-8 9-4-1.5-8-5-8-9V6z"/><path d="m8.5 12 2.5 2.5 4.5-5"/></template>
        <template v-else><rect x="4" y="3" width="16" height="18" rx="2"/><path d="M8 8h8M8 12h8m-8 4h5"/></template>
      </svg>
      <span>{{ sectionLabel(id) }}</span>
    </a>
  </nav>
</template>
