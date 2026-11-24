<script setup lang="ts">
import { computed } from 'vue';
import { logEntryText, type LogLineInput } from '../logPanel';
const props = defineProps<{ lines: LogLineInput[]; empty: string; success?: boolean }>();

interface RenderedLine {
  text: string;
  cls: string;
}
interface LogChunk {
  key: number;
  items: RenderedLine[];
  raw: LogLineInput[];
}

function lineClass(text: string, success?: boolean) {
  if (/ERROR|Error|failed|失败/.test(text)) return 'log-error';
  if (/WARN|Warning/.test(text)) return 'log-warn';
  return success && /成功|Successfully/.test(text) ? 'log-success' : 'log-info';
}

let prevChunks = new Map<number, LogChunk>();
let prevSuccess: boolean | undefined;

function lineSequence(line: LogLineInput, index: number): number {
  return typeof line === 'object' && line !== null && 'entry' in line ? line.seq : index;
}

function sameSlice(raw: LogLineInput[], lines: LogLineInput[], start: number, end: number): boolean {
  if (raw.length !== end - start) return false;
  for (let i = 0; i < raw.length; i += 1) {
    if (raw[i] !== lines[start + i]) return false;
  }
  return true;
}

const chunks = computed(() => {
  const lines = props.lines;
  const success = props.success;
  const reusable = prevSuccess === success;
  const result: LogChunk[] = [];
  for (let i = 0; i < lines.length;) {
    const key = Math.floor(lineSequence(lines[i], i) / 50);
    let end = i + 1;
    while (end < lines.length && Math.floor(lineSequence(lines[end], end) / 50) === key) end += 1;
    const previous = reusable ? prevChunks.get(key) : undefined;
    if (previous && sameSlice(previous.raw, lines, i, end)) {
      result.push(previous);
    } else {
      const raw = lines.slice(i, end);
      const items = raw.map(line => {
        const text = logEntryText(line);
        return { text, cls: lineClass(text, success) };
      });
      result.push({ key, raw, items });
    }
    i = end;
  }
  prevChunks = new Map(result.map(chunk => [chunk.key, chunk]));
  prevSuccess = success;
  return result;
});
</script>
<template>
  <span v-if="!chunks.length" class="log-empty">{{ empty }}</span>
  <div v-for="chunk in chunks" :key="chunk.key" v-memo="[chunk]" class="log-chunk"><div v-for="(line, offset) in chunk.items" :key="offset" :class="line.cls">{{ line.text }}</div></div>
</template>
