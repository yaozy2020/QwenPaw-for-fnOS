<script setup lang="ts">
import LogLines from "./LogLines.vue";
import { usePanel } from "../panelContext";
const panel = usePanel();
const view = panel.logs.view;
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
          <span class="title-meta">qwenpaw.log</span>
        </h2>
      </div>
      <div class="terminal-controls">
        <label class="check-label">
          <input type="checkbox" id="autoScrollCheck" checked> 自动滚底
        </label>
        <label class="check-label">
          <input type="checkbox" id="autoRefreshCheck" checked @change="panel.toggleAutoRefresh(($event.target as HTMLInputElement).checked)"> 实时刷新
        </label>
        <button type="button" class="btn-outline btn-sm" id="exportLogsBtn" :disabled="view.exportBusy" :aria-busy="view.exportBusy || undefined" @click="panel.exportLogs()">导出</button>
        <button type="button" class="btn-outline btn-sm" @click="panel.clearTerminalLogs()">清空日志</button>
      </div>
    </div>
    <div v-if="view.error" role="status" class="notice notice-warn">{{ view.error }} <button type="button" class="btn-outline" @click="panel.logs.refresh()">重试</button></div>
    <div class="terminal-body" id="logTerminal" role="log" aria-label="QwenPaw 运行日志" aria-live="off" tabindex="0"><LogLines :lines="view.lines" :empty="view.error ? '暂时无法读取日志' : '（当前无日志输出）'" /></div>
  </section>
</template>
