<script setup lang="ts">
export type InstallStep = 'downloading' | 'installing' | 'starting';
export interface InstallStepsState { current: InstallStep | null; outcome: 'done' | 'failed' | null }
const props = defineProps<InstallStepsState>();
const steps: [InstallStep, string][] = [['downloading', '下载依赖'], ['installing', '安装内核'], ['starting', '启动检查']];
function status(index: number) {
  const reached = steps.findIndex(([step]) => step === props.current);
  return props.outcome === 'done' || index < reached ? 'done' : index === reached ? props.outcome || 'active' : 'pending';
}
</script>
<template>
  <li v-for="([step, label], index) in steps" :key="step" :data-step="step" :data-status="status(index)" :aria-current="status(index) === 'active' ? 'step' : undefined"><span class="step-mark" aria-hidden="true"></span>{{ label }}</li>
</template>
