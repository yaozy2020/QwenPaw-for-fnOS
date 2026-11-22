<script setup lang="ts">
import { t, type MessageKey } from '../i18n';
export type InstallStep = 'downloading' | 'installing' | 'starting';
export interface InstallStepsState { current: InstallStep | null; outcome: 'done' | 'failed' | null }
const props = defineProps<InstallStepsState>();
const steps: [InstallStep, MessageKey][] = [
  ['downloading', 'progress.step.downloading'],
  ['installing', 'progress.step.installing'],
  ['starting', 'progress.step.starting'],
];
function status(index: number) {
  const reached = steps.findIndex(([step]) => step === props.current);
  return props.outcome === 'done' || index < reached ? 'done' : index === reached ? props.outcome || 'active' : 'pending';
}
</script>
<template>
  <li v-for="([step, labelKey], index) in steps" :key="step" :data-step="step" :data-status="status(index)" :aria-current="status(index) === 'active' ? 'step' : undefined"><span class="step-mark" aria-hidden="true"></span>{{ t(labelKey) }}</li>
</template>
