<script setup lang="ts">
import { computed } from 'vue';
import type { LogEntry } from '../api';
const props = defineProps<{ lines: LogEntry[]; empty: string; success?: boolean }>();
const chunks = computed(() => {
  const result: string[][] = [];
  for (let i = 0; i < props.lines.length; i += 50) {
    result.push(props.lines.slice(i, i + 50).map(line => typeof line === 'string' ? line : line.message || JSON.stringify(line)));
  }
  return result;
});
function lineClass(text: string) {
  if (/ERROR|Error|failed|失败/.test(text)) return 'log-error';
  if (/WARN|Warning/.test(text)) return 'log-warn';
  return props.success && /成功|Successfully/.test(text) ? 'log-success' : 'log-info';
}
</script>
<template>
  <span v-if="!chunks.length" class="log-empty">{{ empty }}</span>
  <div v-for="(chunk, index) in chunks" :key="index" class="log-chunk"><div v-for="(line, offset) in chunk" :key="offset" :class="lineClass(line)">{{ line }}</div></div>
</template>
