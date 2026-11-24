<script setup lang="ts">
import { computed } from 'vue';
import { logEntryText, type LogLineInput } from '../logPanel';
const props = defineProps<{ lines: LogLineInput[]; empty: string; success?: boolean; highlight?: string }>();

interface HighlightPart {
  text: string;
  match: boolean;
}
interface RenderedLine {
  text: string;
  cls: string;
  parts?: HighlightPart[];
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

function splitHighlight(text: string, keyword: string): HighlightPart[] | undefined {
  if (!keyword) return undefined;
  const lower = text.toLowerCase();
  const len = keyword.length;
  let index = lower.indexOf(keyword);
  if (index === -1) return undefined;
  const parts: HighlightPart[] = [];
  let cursor = 0;
  while (index !== -1) {
    if (index > cursor) parts.push({ text: text.slice(cursor, index), match: false });
    parts.push({ text: text.slice(index, index + len), match: true });
    cursor = index + len;
    index = lower.indexOf(keyword, cursor);
  }
  if (cursor < text.length) parts.push({ text: text.slice(cursor), match: false });
  return parts;
}

let prevChunks = new Map<number, LogChunk>();
let prevSuccess: boolean | undefined;
let prevHighlight = '';

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
  const highlight = props.highlight?.trim().toLowerCase() ?? '';
  const reusable = prevSuccess === success && prevHighlight === highlight;
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
        const parts = splitHighlight(text, highlight);
        return parts ? { text, cls: lineClass(text, success), parts } : { text, cls: lineClass(text, success) };
      });
      result.push({ key, raw, items });
    }
    i = end;
  }
  prevChunks = new Map(result.map(chunk => [chunk.key, chunk]));
  prevSuccess = success;
  prevHighlight = highlight;
  return result;
});
</script>
<template>
  <span v-if="!chunks.length" class="log-empty">{{ empty }}</span>
  <div v-for="chunk in chunks" :key="chunk.key" v-memo="[chunk]" class="log-chunk"><div v-for="(line, offset) in chunk.items" :key="offset" :class="line.cls"><template v-if="line.parts"><template v-for="(part, partIndex) in line.parts" :key="partIndex"><mark v-if="part.match" class="log-highlight">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template></template><template v-else>{{ line.text }}</template></div></div>
</template>
