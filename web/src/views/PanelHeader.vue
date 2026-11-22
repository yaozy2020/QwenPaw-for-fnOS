<script setup lang="ts">
import { computed } from "vue";
import PawIcon from './PawIcon.vue';
import LocaleToggle from './LocaleToggle.vue';
import ThemeToggle from './ThemeToggle.vue';
import { usePanel } from "../panelContext";
import { t } from "../i18n";
const emit = defineEmits<{ about: [] }>();
const panel = usePanel();
const status = panel.status.view;
const entry = computed(() => !status.data ? ['#overview', t('header.entry.connecting'), t('header.entry.connectingShort')]
  : !status.reachable ? ['#overview', t('header.entry.check'), t('header.entry.checkShort')]
  : status.recovery ? ['#overview', t('header.entry.recovery'), t('header.entry.recoveryShort')]
  : !status.data.installed ? ['#version', t('header.entry.install'), t('header.entry.installShort')]
  : !status.data.running ? ['#overview', t('header.entry.start'), t('header.entry.startShort')]
  : ['/app/qwenpaw/web/', t('header.entry.chat'), t('header.entry.chatShort')]);
const badge = computed(() => !status.data ? ['warning', t('header.badge.checking')]
  : !status.reachable ? ['warning', t('header.badge.unreachable')]
  : status.recovery ? ['warning', t('header.badge.recovery')]
  : !status.data.installed ? ['warning', t('header.badge.uninstalled')]
  : status.data.running ? ['success', t('header.badge.running')]
  : ['danger', t('header.badge.stopped')]);
</script>

<template>
  <header>
    <div class="brand">
      <div class="brand-icon">
        <PawIcon :size="26" />
      </div>
      <div class="brand-info">
        <div class="brand-title-row">
          <h1 id="sectionTitle">{{ panel.navigation.title }}</h1>
          <span id="statusBadge" class="badge" :class="`badge-${badge[0]}`"><span class="pulse-dot"></span> {{ badge[1] }}</span>
        </div>
      </div>
    </div>
    <div class="header-actions">
      <LocaleToggle />
      <ThemeToggle />
      <button type="button" class="btn-outline" id="aboutButton" @click="emit('about')" :title="t('header.aboutTitle')" :aria-label="t('header.aboutTitle')" aria-haspopup="dialog" aria-controls="aboutDialog">
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7h.01"/>
        </svg>
        <span class="btn-label">{{ t('header.about') }}</span>
      </button>
      <button type="button" id="taskCenterButton" @click="panel.showTaskDetails()" class="btn btn-secondary" :hidden="!panel.progress.view.owned" :data-short-label="t('header.taskCenterShort')" :aria-label="t('header.taskCenterAria')" aria-haspopup="dialog" aria-controls="upgradeModal" :aria-expanded="panel.progress.view.open">{{ t('header.taskCenter') }}</button>
      <a id="btnWebConsole" class="btn btn-primary" data-web-console @click="panel.openConsole" :href="entry[0]" :aria-label="entry[1]">
        <span id="webConsoleText">{{ entry[1] }}</span>
        <span id="webConsoleMobileText" aria-hidden="true">{{ entry[2] }}</span>
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <path d="M5 12h14m-7-7 7 7-7 7"/>
        </svg>
      </a>
    </div>
  </header>
</template>
