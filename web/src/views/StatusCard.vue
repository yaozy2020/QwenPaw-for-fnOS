<script setup lang="ts">
import { computed } from 'vue';
import { formatMem, type StatusData } from '../statusStore';
import { currentLocale, t } from '../i18n';
const props = defineProps<{ data: StatusData; uptime: string }>();
const unavailable = computed(() => props.data.degraded);
const running = computed(() => !unavailable.value && props.data.installed !== false && props.data.running);
const state = computed(() => unavailable.value || props.data.recoveryReason ? 'degraded' : props.data.installed === false ? 'uninstalled' : running.value ? 'running' : 'stopped');
const title = computed(() => unavailable.value ? t('status.unreachable') : props.data.recoveryReason ? t('status.recovery') : props.data.installed === false ? t('status.uninstalled') : running.value ? t('status.running') : t('status.stopped'));
const description = computed(() => {
  const { degradedDetail, recoveryReason, installed } = props.data;
  if (unavailable.value) {
    const detail = degradedDetail ? (currentLocale.value === 'en' ? ` (${degradedDetail})` : `（${degradedDetail}）`) : '';
    return t('status.desc.unreachable', { detail });
  }
  if (recoveryReason) return t('status.desc.recovery', { reason: recoveryReason });
  if (installed === false) return t('status.desc.uninstalled');
  return running.value ? t('status.desc.running') : t('status.desc.stopped');
});
const stats = computed(() => {
  const { cpu, memKB, pid } = props.data;
  return [
    ['cpu', t('overview.cpu'), running.value && typeof cpu === 'number' ? `${cpu.toFixed(1)}%` : '-'],
    ['mem', t('overview.memory'), running.value ? formatMem(memKB) : '-'],
    ['pid', t('overview.pid'), running.value ? pid : '-'],
    ['uptime', t('overview.uptime'), running.value && props.uptime ? props.uptime : '-'],
  ];
});
</script>
<template>
  <div class="status-summary">
    <div class="status-head" :data-state="state"><span class="status-dot" aria-hidden="true"></span><span class="status-text">{{ title }}</span></div>
    <p class="status-description" aria-live="polite">{{ description }}</p>
    <a v-if="data.recoveryReason && !unavailable" href="#logs">{{ t('status.viewLogs') }}</a>
  </div>
  <div class="stat-grid" :aria-label="t('overview.metricsAria')">
    <div v-for="[key, label, value] in stats" :key="key" class="stat"><div class="stat-label">{{ label }}</div><div class="stat-value">{{ value }}</div></div>
  </div>
</template>
