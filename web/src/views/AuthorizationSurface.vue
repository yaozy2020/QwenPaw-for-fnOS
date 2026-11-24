<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { usePanel } from "../panelContext";
import { t } from "../i18n";

const panel = usePanel();
const view = panel.dirs.state.surface;
const cancel = ref<HTMLButtonElement>();
let previousFocus: HTMLElement | null = null;

watch([() => view.open, () => panel.dirs.state.busy], async ([open, busy]) => {
  if (open) {
    previousFocus = view.trigger ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    await nextTick();
    if (view.open) cancel.value?.focus();
    return;
  }
  if (busy) return;
  const target = previousFocus;
  requestAnimationFrame(() => {
    if (view.open || panel.dirs.state.busy) return;
    previousFocus = null;
    if (target?.isConnected) target.focus({ preventScroll: true });
  });
}, { flush: "post" });
</script>

<template>
  <Teleport to="body">
    <div v-if="view.open" id="authorizationSurface" class="authorization-surface" role="dialog" aria-modal="true" :aria-label="t('authz.surfaceTitle')" tabindex="-1" @keydown.esc.prevent="view.close()">
      <div class="authorization-surface-bar">
        <strong>{{ t('authz.surfaceTitle') }}</strong>
        <button ref="cancel" type="button" class="authorization-surface-cancel" @click="view.close()">{{ t('common.cancel') }}</button>
      </div>
      <iframe class="authorization-surface-frame" :src="view.url" :title="t('authz.surfaceTitle')" />
    </div>
  </Teleport>
</template>
