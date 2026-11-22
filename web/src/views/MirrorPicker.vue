<script setup lang="ts">
import { nextTick, ref, watch } from 'vue';
import OptionList from './OptionList.vue';
import { useOutsideDismiss } from './dialogHelpers';
import { usePanel } from '../panelContext';
import { t } from '../i18n';

const panel = usePanel();
const mirror = panel.settings.mirror;
const root = ref<HTMLElement>();
const trigger = ref<HTMLButtonElement>();
const list = ref<HTMLElement>();

function buttons(): HTMLButtonElement[] {
  return Array.from(list.value?.querySelectorAll<HTMLButtonElement>('button') ?? []);
}
async function focusOption(index?: number) {
  await nextTick();
  const items = buttons();
  if (!items.length) return;
  const target = index !== undefined
    ? Math.max(0, Math.min(index, items.length - 1))
    : Math.max(0, panel.settings.mirrors.findIndex(m => m.value === mirror.value));
  items[target]?.focus({ preventScroll: true });
}
function openWithKeyboard() {
  if (mirror.saving) return;
  if (!mirror.open) panel.toggleMirrorDropdown();
  void focusOption();
}
function close(restore = false) {
  if (!mirror.open) return;
  mirror.open = false;
  if (restore) trigger.value?.focus({ preventScroll: true });
}
function move(event: KeyboardEvent) {
  const items = buttons();
  if (!items.length) return;
  const current = items.indexOf(document.activeElement as HTMLButtonElement);
  const next = {
    ArrowDown: current < 0 ? 0 : current + 1,
    ArrowUp: current < 0 ? items.length - 1 : current - 1,
    Home: 0,
    End: items.length - 1,
  }[event.key];
  if (next !== undefined) {
    event.preventDefault();
    void focusOption(next);
  }
}
function pick(value: string) {
  void panel.settings.pickMirror(value);
  trigger.value?.focus({ preventScroll: true });
}
const { onFocusOut } = useOutsideDismiss(root, () => mirror.open, () => close());
watch(() => mirror.open, shown => {
  if (!shown) return;
  void nextTick(() => list.value?.scrollIntoView({ block: 'nearest' }));
});
watch(() => mirror.saving || panel.navigation.hides('overview'), hidden => { if (hidden) close(); });
</script>

<template>
  <div class="field-row">
    <span class="field-label" id="mirrorFieldLabel">{{ t('mirror.label') }}</span>
    <div class="field-control" ref="root" @focusout="onFocusOut" @keydown.esc.stop.prevent="close(true)">
      <button ref="trigger" type="button" class="select-btn" id="mirrorToggleBtn" aria-labelledby="mirrorFieldLabel mirrorLabel" :aria-expanded="mirror.open" :disabled="mirror.saving" aria-controls="mirrorDropdown" @click="panel.toggleMirrorDropdown()" @keydown.down.prevent="openWithKeyboard" @keydown.up.prevent="openWithKeyboard">
        <span id="mirrorLabel">{{ mirror.ready ? panel.settings.mirrors.find(m => m.value === mirror.value)?.label || mirror.value : t('mirror.view') }}</span>
        <span class="caret" aria-hidden="true">▾</span>
      </button>
      <div ref="list" class="history-dropdown float" id="mirrorDropdown" :class="{ show: mirror.open }" @keydown="move">
        <OptionList :options="panel.settings.mirrors" :selected="mirror.value" @pick="pick" />
      </div>
    </div>
  </div>
</template>
