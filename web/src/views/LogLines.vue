<script setup lang="ts">
import { computed } from 'vue';
import type { LogEntry } from '../api';
const props = defineProps<{ lines: LogEntry[]; empty: string; success?: boolean }>();

interface RenderedLine {
  text: string;
  cls: string;
}
interface LogChunk {
  items: RenderedLine[];
  raw: LogEntry[];
}

function lineClass(text: string, success?: boolean) {
  if (/ERROR|Error|failed|失败/.test(text)) return 'log-error';
  if (/WARN|Warning/.test(text)) return 'log-warn';
  return success && /成功|Successfully/.test(text) ? 'log-success' : 'log-info';
}

let prevChunks: LogChunk[] = [];
let prevSuccess: boolean | undefined;

function sameSlice(raw: LogEntry[], lines: LogEntry[], start: number, end: number): boolean {
  if (raw.length !== end - start) return false;
  for (let i = 0; i < raw.length; i += 1) {
    if (raw[i] !== lines[start + i]) return false;
  }
  return true;
}

const chunks = computed(() => {
  const lines = props.lines;
  const success = props.success;
  const reuse = prevSuccess === success;
  const result: LogChunk[] = [];
  for (let i = 0, chunkIndex = 0; i < lines.length; i += 50, chunkIndex += 1) {
    const end = Math.min(i + 50, lines.length);
    const previous = reuse ? prevChunks[chunkIndex] : undefined;
    if (previous && sameSlice(previous.raw, lines, i, end)) {
      result.push(previous);
      continue;
    }
    const raw = lines.slice(i, end);
    const items = raw.map(line => {
      const text = typeof line === 'string' ? line : line.message || JSON.stringify(line);
      return { text, cls: lineClass(text, success) };
    });
    result.push({ raw, items });
  }
  prevChunks = result;
  prevSuccess = success;
  return result;
});
</script>
<template>
  <span v-if="!chunks.length" class="log-empty">{{ empty }}</span>
  <div v-for="(chunk, index) in chunks" :key="index" v-memo="[chunk]" class="log-chunk"><div v-for="(line, offset) in chunk.items" :key="offset" :class="line.cls">{{ line.text }}</div></div>
</template>
