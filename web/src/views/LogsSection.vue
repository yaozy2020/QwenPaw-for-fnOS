<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import LogLines from "./LogLines.vue";
import { matchesLogFilter, type LogLevelFilter } from "../logPanel";
import { usePanel } from "../panelContext";
const panel = usePanel();
const view = panel.logs.view;
const levels: { value: LogLevelFilter; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "warn", label: "警告+" },
  { value: "error", label: "错误" },
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
  if (view.error) return "暂时无法读取日志";
  if (filtering.value && view.lines.length) return "当前筛选条件下无匹配日志";
  return "（当前无日志输出）";
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
          运行日志
        </h2>
      </div>
      <button type="button" class="btn-outline btn-sm" id="exportLogsBtn" :disabled="view.exportBusy" :aria-busy="view.exportBusy || undefined" @click="panel.exportLogs()">导出</button>
    </div>
    <div class="log-source-bar">
      <span class="log-file-name" title="QwenPaw 运行日志文件">qwenpaw.log</span>
      <label class="check-label log-follow-control">
        <input type="checkbox" id="autoScrollCheck" checked> 自动滚底
      </label>
    </div>
    <details class="log-tools" id="logTools">
      <summary>
        <span>筛选与管理</span>
        <span v-if="filtering" class="log-tools-active">{{ visibleLines.length }} / {{ view.lines.length }} 行</span>
      </summary>
      <div class="log-tools-content">
        <div class="terminal-controls">
          <label class="check-label">
            <input type="checkbox" id="autoRefreshCheck" checked @change="panel.toggleAutoRefresh(($event.target as HTMLInputElement).checked)"> 实时刷新
          </label>
          <button type="button" class="btn-outline btn-sm" id="copyLogsBtn" :disabled="!visibleLines.length" @click="panel.copyLogs(visibleLines)">复制当前结果</button>
          <button type="button" class="btn-outline btn-sm" id="clearLogsBtn" @click="panel.clearTerminalLogs()">清空日志</button>
        </div>
        <search class="log-filter-bar" aria-label="日志筛选">
          <div class="log-level-group" role="group" aria-label="日志级别">
            <button v-for="item in levels" :key="item.value" type="button" class="btn-outline btn-xs log-level-btn" :aria-pressed="view.level === item.value" :data-level="item.value" @click="view.level = item.value">{{ item.label }}</button>
          </div>
          <div class="log-search-wrap">
            <label for="logSearchInput" class="log-search-label">关键字筛选</label>
            <input id="logSearchInput" type="search" name="log-query" class="log-search-input" v-model="queryInput" placeholder="输入日志关键字" autocomplete="off" autocorrect="off" autocapitalize="none" :spellcheck="false" />
            <button v-if="filtering || queryInput.trim()" type="button" class="btn-outline btn-xs" id="clearLogFilterBtn" @click="resetFilter">重置</button>
          </div>
          <span v-if="filtering" id="logFilterCount" class="card-note" aria-live="polite">显示 {{ visibleLines.length }} / {{ view.lines.length }} 行</span>
        </search>
      </div>
    </details>
    <div v-if="view.error" role="status" class="notice notice-warn">{{ view.error }} <button type="button" class="btn-outline" @click="panel.logs.refresh()">重试</button></div>
    <div class="terminal-body" id="logTerminal" role="log" aria-label="QwenPaw 运行日志" aria-live="off" tabindex="0"><LogLines :lines="visibleLines" :empty="emptyText" /></div>
  </section>
</template>
