<script setup lang="ts">
import { computed } from 'vue';
import { formatMem, type StatusData } from '../statusStore';
const props = defineProps<{ data: StatusData; uptime: string }>();
const unavailable = computed(() => props.data.degraded);
const running = computed(() => !unavailable.value && props.data.installed !== false && props.data.running);
const state = computed(() => unavailable.value || props.data.recoveryReason ? 'degraded' : props.data.installed === false ? 'uninstalled' : running.value ? 'running' : 'stopped');
const title = computed(() => unavailable.value ? '状态不可达' : props.data.recoveryReason ? '需要恢复' : props.data.installed === false ? '内核未安装' : running.value ? '运行中' : '已停止');
const description = computed(() => {
  const { degradedDetail, recoveryReason, installed } = props.data;
  if (unavailable.value) return `暂时无法获取服务状态，请检查网络或重新登录飞牛后重试。${degradedDetail ? `（${degradedDetail}）` : ''}`;
  if (recoveryReason) return `暂勿重新安装或启停服务。请查看运行日志并保留现场。原因：${recoveryReason}`;
  if (installed === false) return '先安装 QwenPaw 内核，即可开始使用 AI 对话。';
  return running.value ? '内核进程正在运行，可在下方查看资源占用和运行时间。' : '内核已安装，启动服务后即可继续对话。';
});
const stats = computed(() => {
  const { cpu, memKB, pid } = props.data;
  return [
    ['CPU 使用率', running.value && typeof cpu === 'number' ? `${cpu.toFixed(1)}%` : '-'],
    ['内存占用', running.value ? formatMem(memKB) : '-'],
    ['进程 PID', running.value ? pid : '-'],
    ['运行时间', running.value && props.uptime ? props.uptime : '-'],
  ];
});
</script>
<template>
  <div class="status-summary">
    <div class="status-head" :data-state="state"><span class="status-dot" aria-hidden="true"></span><span class="status-text">{{ title }}</span></div>
    <p class="status-description" aria-live="polite">{{ description }}</p>
    <a v-if="data.recoveryReason && !unavailable" href="#logs">查看运行日志</a>
  </div>
  <div class="stat-grid" aria-label="运行指标">
    <div v-for="[label, value] in stats" :key="label" class="stat"><div class="stat-label">{{ label }}</div><div class="stat-value">{{ value }}</div></div>
  </div>
</template>
