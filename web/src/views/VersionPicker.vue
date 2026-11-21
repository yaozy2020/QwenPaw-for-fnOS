<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import { usePanel } from '../panelContext';

const panel = usePanel();
const v = panel.versions.view;
const root = ref<HTMLElement>();
const trigger = ref<HTMLButtonElement>();
const list = ref<HTMLElement>();
const open = ref(false);
const active = ref(0);
const disabled = computed(() => !v.fresh || !v.list.length);
const selected = computed(() => v.list.find(item => item.version === v.selected));
const hint = computed(() => v.loading ? '正在读取版本…' : v.list.length ? '请选择目标版本' : '点「检查更新」获取版本列表');

async function focusRow(index: number) {
  active.value = Math.max(0, Math.min(index, v.list.length - 1));
  await nextTick();
  list.value?.querySelector<HTMLElement>('[tabindex="0"]')?.focus({ preventScroll: true });
  const row = list.value?.children[active.value] as HTMLElement | undefined;
  if (list.value && row) {
    const offset = row.getBoundingClientRect().top - list.value.getBoundingClientRect().top;
    if (offset < 0) list.value.scrollTop += offset;
    else if (offset + row.offsetHeight > list.value.clientHeight) list.value.scrollTop += offset + row.offsetHeight - list.value.clientHeight;
  }
}
function show() {
  if (disabled.value) return;
  open.value = true;
  void focusRow(v.list.findIndex(item => item.version === v.selected));
}
function close(restore = false) {
  open.value = false;
  if (restore) trigger.value?.focus({ preventScroll: true });
}
function pick(version: string) {
  panel.versions.pickVersion(version);
  close(true);
}
function move(event: KeyboardEvent) {
  const next = { ArrowDown: active.value + 1, ArrowUp: active.value - 1, Home: 0, End: v.list.length - 1 }[event.key];
  if (next !== undefined) { event.preventDefault(); void focusRow(next); }
}
function dismiss(event: PointerEvent) {
  if (event.target instanceof Node && !root.value?.contains(event.target)) close();
}
function blur(event: FocusEvent) {
  if (event.relatedTarget instanceof Node && !root.value?.contains(event.relatedTarget)) close();
}
watch(open, (shown, _, cleanup) => {
  if (!shown) return;
  document.addEventListener('pointerdown', dismiss, true);
  cleanup(() => document.removeEventListener('pointerdown', dismiss, true));
});
watch(() => disabled.value || panel.navigation.hides('overview'), hidden => { if (hidden) close(); });
</script>

<template>
  <div class="version-field" ref="root" @focusout="blur" @keydown.esc.stop.prevent="close(true)">
    <span id="targetVersionLabel" class="field-label">目标版本</span>
    <button ref="trigger" type="button" class="select-btn release-trigger" id="targetVersion" aria-labelledby="targetVersionLabel targetVersionText" aria-haspopup="listbox" :aria-expanded="open" :aria-controls="open ? 'versionOptions' : undefined" aria-describedby="versionOrder" :disabled="disabled" @click="open ? close() : show()" @keydown.down.prevent="show" @keydown.up.prevent="show">
      <span id="targetVersionText"><span :class="{ 'history-version': selected }">{{ selected ? 'v' + selected.version : hint }}</span><span v-if="selected" class="ver-tag-pill" :class="selected.is_beta ? 'beta' : 'stable'">{{ selected.is_beta ? '测试版' : '正式版' }}</span></span><span class="caret" aria-hidden="true">▾</span>
    </button>
    <div v-if="open" ref="list" id="versionOptions" class="history-dropdown show release-options" role="listbox" aria-labelledby="targetVersionLabel" @keydown="move">
      <button v-for="(item, i) in v.list" :key="item.version" type="button" role="option" class="history-item" :tabindex="active === i ? 0 : -1" :aria-selected="item.version === v.selected" :data-version="item.version" @click="pick(item.version)">
        <span class="history-version">v{{ item.version }}</span><span class="release-tags"><span class="ver-tag-pill" :class="item.is_beta ? 'beta' : 'stable'">{{ item.is_beta ? '测试版' : '正式版' }}</span><span v-if="item.version === v.recommended">随包适配补丁</span><span v-if="item.version === v.current">当前安装</span></span><span class="release-check" aria-hidden="true">{{ item.version === v.selected ? '✓' : '' }}</span>
      </button>
    </div>
    <slot />
  </div>
</template>
