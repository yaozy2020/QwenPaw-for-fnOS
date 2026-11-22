<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import LogLines from "./LogLines.vue";
import { matchesLogFilter, type LogLevelFilter } from "../logPanel";
import { usePanel } from "../panelContext";
import { t, type MessageKey } from "../i18n";
const panel = usePanel();
const view = panel.logs.view;
const levelSpecs: { value: LogLevelFilter; labelKey: MessageKey }[] = [
  { value: "all", labelKey: "logs.level.all" },
  { value: "warn", labelKey: "logs.level.warn" },
  { value: "error", labelKey: "logs.level.error" },
];
const queryInput = ref(view.query);
let queryTimer: ReturnType<typeof setTimeout> | null = null;
watch(queryInput, next => {
  if (queryTimer !== null) clearTimeout(queryTimer);
  if (!next.trim()) {
    view.query = "";
    queryTimer = null;
    return;
  }
  queryTimer = setTimeout(() => {
    view.query = next;
    queryTimer = null;
  }, 120);
});
onUnmounted(() => {
  if (queryTimer !== null) {
    clearTimeout(queryTimer);
    queryTimer = null;
  }
});
function resetFilter() {
  if (queryTimer !== null) {
    clearTimeout(queryTimer);
    queryTimer = null;
  }
  view.level = "all";
  queryInput.value = "";
  view.query = "";
}
const normalizedQuery = computed(() => view.query.trim().toLowerCase());
const filtering = computed(() => view.level !== "all" || normalizedQuery.value.length > 0);
const visibleLines = computed(() => {
  if (!filtering.value) return view.lines;
  const level = view.level;
  const keyword = normalizedQuery.value;
  return view.lines.filter(entry => matchesLogFilter(entry, level, keyword, true));
});
const emptyText = computed(() => {
  if (view.error) return t("logs.emptyError");
  if (filtering.value && view.lines.length) return t("logs.emptyFiltered");
  return t("logs.empty");
});
</script>

<template>
  <section class="card terminal-card" id="logs" tabindex="-1" data-mobile-panel="logs" :hidden="panel.navigation.hides('logs')" aria-labelledby="logsTitle">
    <div class="terminal-header">
      <div class="card-title">
        <h2 class="card-title-left" id="logsTitle">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <polyline points="4 17 10 11 4 5"/>
            <line x1="12" y1="19" x2="20" y2="19"/>
          </svg>
          {{ t('logs.title') }}
        </h2>
      </div>
      <div class="log-source-bar">
        <span class="log-file-name" :title="t('logs.fileTitle')">qwenpaw.log</span>
        <div class="log-header-actions">
          <label class="check-label log-follow-control">
            <input type="checkbox" id="autoScrollCheck" v-model="view.autoScroll" @change="panel.logs.setAutoScroll(view.autoScroll)"> {{ t('logs.autoScroll') }}
          </label>
          <button type="button" class="btn-outline btn-sm" id="exportLogsBtn" :disabled="view.exportBusy" :aria-busy="view.exportBusy || undefined" @click="panel.exportLogs()">{{ t('logs.export') }}</button>
        </div>
      </div>
    </div>
    <details class="log-tools" id="logTools">
      <summary>
        <span>{{ t('logs.tools') }}</span>
        <span v-if="filtering" class="log-tools-active">
          <span>{{ t('logs.lineCount', { visible: visibleLines.length, total: view.lines.length }) }}</span>
          <button type="button" class="btn-outline btn-xs log-tools-clear" @click.stop.prevent="resetFilter">{{ t('logs.clearFilter') }}</button>
        </span>
      </summary>
      <div class="log-tools-content">
        <div class="terminal-controls">
          <label class="check-label">
            <input type="checkbox" id="autoRefreshCheck" checked @change="panel.toggleAutoRefresh(($event.target as HTMLInputElement).checked)"> {{ t('logs.autoRefresh') }}
          </label>
          <button type="button" class="btn-outline btn-sm" id="copyLogsBtn" :disabled="!visibleLines.length" @click="panel.copyLogs(visibleLines)">{{ t('logs.copyFiltered') }}</button>
          <button type="button" class="btn-outline btn-sm" id="clearLogsBtn" @click="panel.clearTerminalLogs()">{{ t('logs.clearLogs') }}</button>
        </div>
        <search class="log-filter-bar" :aria-label="t('logs.filterAria')">
          <div class="log-level-group" role="group" :aria-label="t('logs.levelAria')">
            <button v-for="item in levelSpecs" :key="item.value" type="button" class="btn-outline btn-xs log-level-btn" :aria-pressed="view.level === item.value" :data-level="item.value" @click="view.level = item.value">{{ t(item.labelKey) }}</button>
          </div>
          <div class="log-search-wrap">
            <label for="logSearchInput" class="log-search-label">{{ t('logs.keywordLabel') }}</label>
            <input id="logSearchInput" type="search" name="log-query" class="log-search-input" v-model="queryInput" :placeholder="t('logs.keywordPlaceholder')" autocomplete="off" autocorrect="off" autocapitalize="none" :spellcheck="false" />
            <button v-if="filtering || queryInput.trim()" type="button" class="btn-outline btn-xs" id="clearLogFilterBtn" @click="resetFilter">{{ t('logs.resetFilter') }}</button>
          </div>
          <span v-if="filtering" id="logFilterCount" class="card-note" aria-live="polite">{{ t('logs.showingCount', { visible: visibleLines.length, total: view.lines.length }) }}</span>
        </search>
      </div>
    </details>
    <div v-if="view.error" role="status" class="notice notice-warn">{{ view.error }} <button type="button" class="btn-outline" @click="panel.logs.refresh()">{{ t('logs.retry') }}</button></div>
    <div class="terminal-body" id="logTerminal" role="log" :aria-label="t('logs.terminalAria')" aria-live="off" tabindex="0"><LogLines :lines="visibleLines" :empty="emptyText" /></div>
  </section>
</template>
