<script setup lang="ts">
import { computed } from "vue";
import ThemeToggle from './ThemeToggle.vue';
import { usePanel } from "../panelContext";
const emit = defineEmits<{ about: [] }>();
const panel = usePanel();
const status = panel.status.view;
const entry = computed(() => !status.data ? ['#overview', '正在连接服务', '连接']
  : !status.reachable ? ['#overview', '检查服务状态', '检查'] : status.recovery ? ['#overview', '查看恢复原因', '恢复']
  : !status.data.installed ? ['#version', '安装并开始使用', '安装'] : !status.data.running ? ['#overview', '先启动服务', '启动']
  : ['/app/qwenpaw/web/', '进入 Web 对话', '对话']);
const badge = computed(() => !status.data ? ['warning', '检测中…'] : !status.reachable ? ['warning', '状态不可达']
  : status.recovery ? ['warning', '需要恢复'] : !status.data.installed ? ['warning', '内核未安装']
  : status.data.running ? ['success', '运行中'] : ['danger', '已停止']);
</script>

<template>
  <header>
    <div class="brand">
      <div class="brand-icon">
        <svg viewBox="0 0 24 24" width="26" height="26" fill="currentColor">
          <circle cx="8.5" cy="5.5" r="2.5"/>
          <circle cx="15.5" cy="5.5" r="2.5"/>
          <circle cx="4.5" cy="11.5" r="2.5"/>
          <circle cx="19.5" cy="11.5" r="2.5"/>
          <path d="M12 11c-3.5 0-6 2.5-6 6.5 0 2.2 1.8 4 4 4 1.2 0 2.4-.6 3-1.6.6 1 1.8 1.6 3 1.6 2.2 0 4-1.8 4-4 0-4-2.5-6.5-6-6.5z"/>
        </svg>
      </div>
      <div class="brand-info">
        <div class="brand-title-row">
          <h1 id="sectionTitle">{{ panel.navigation.title }}</h1>
          <span id="statusBadge" class="badge" :class="`badge-${badge[0]}`"><span class="pulse-dot"></span> {{ badge[1] }}</span>
        </div>
      </div>
    </div>
    <div class="header-actions">
      <ThemeToggle />
      <button type="button" class="btn-outline" id="aboutButton" @click="emit('about')" title="关于与常见问题" aria-label="关于与常见问题" aria-haspopup="dialog" aria-controls="aboutDialog">
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7h.01"/>
        </svg>
        <span class="btn-label">关于</span>
      </button>
      <button type="button" id="taskCenterButton" @click="panel.showTaskDetails()" class="btn btn-secondary" :hidden="!panel.progress.view.owned" aria-label="查看安装任务详情" aria-haspopup="dialog" aria-controls="upgradeModal" :aria-expanded="panel.progress.view.open">查看安装任务</button>
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
