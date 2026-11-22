<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import LogLines from "./LogLines.vue";
import InstallSteps from "./InstallSteps.vue";
import { closeDialog, showDialog } from "./dialogHelpers";
import { usePanel } from "../panelContext";
import { t } from "../i18n";

const panel = usePanel();
const view = panel.progress.view;
const dialog = ref<HTMLDialogElement>();
const box = ref<HTMLElement>();
const minimizeBtn = ref<HTMLButtonElement>();
const closeBtn = ref<HTMLButtonElement>();
let opened = 0;

function dismiss() {
  if (view.minimize) panel.minimizeUpgradeModal();
  else panel.closeUpgradeModal();
}

function trapFocus(event: KeyboardEvent) {
  if (event.key !== "Tab" || !dialog.value) return;
  const el = dialog.value;
  const focusable = Array.from(el.querySelectorAll<HTMLElement>('button, input, [href], select, textarea, [tabindex]:not([tabindex="-1"])'))
    .filter(item => !item.matches(':disabled') && item.getClientRects().length > 0);
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (!first || !last) {
    event.preventDefault();
    el.focus();
  } else if (!el.contains(document.activeElement) || (event.shiftKey && document.activeElement === first)) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

watch(() => view.open, async open => {
  const el = dialog.value;
  if (!open) {
    closeDialog(el);
    return;
  }
  await nextTick();
  if (!view.open) return;
  opened = Date.now();
  showDialog(el);
  await nextTick();
  if (!view.open) return;
  const target = view.minimize ? minimizeBtn.value : closeBtn.value;
  target?.focus();
}, { flush: "post" });
</script>

<template>
  <dialog ref="dialog" class="modal-overlay" id="upgradeModal" :class="{active: view.open}" aria-labelledby="upTitle" aria-describedby="upSubTitle" tabindex="-1" @keydown="trapFocus" @cancel.prevent="dismiss" @click.self="Date.now() - opened > 250 && dismiss()">
    <div ref="box" class="modal-box">
      <div class="modal-header">
        <div class="modal-icon-badge primary">
          <span class="spinner" id="upSpinner" v-show="view.spinner"></span>
          <svg id="upDoneIcon" viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2.5" v-show="view.icon" :style="{color: view.success ? 'var(--success)' : 'var(--danger-text)'}">
            <polyline points="20 6 9 17 4 12"/>
          </svg>
        </div>
        <div class="modal-heading">
          <div class="modal-title" id="upTitle">{{ view.title }}</div>
          <div class="modal-subtitle" id="upSubTitle">{{ view.subtitle }}</div>
        </div>
      </div>
      <ol class="install-steps" id="upSteps" :aria-label="t('progress.stepsAria')">
        <InstallSteps v-bind="view.steps" />
      </ol>
      <div class="upgrade-terminal-body" id="upTerminal"><LogLines :lines="view.lines" :empty="view.empty" success /></div>
      <div class="modal-actions">
        <!-- 常驻出口:任务执行中也能关掉弹窗(转后台继续跑),
             避免后端失联时页面被永久锁死 -->
        <button ref="minimizeBtn" class="btn-outline btn-block" id="btnUpMinimize" v-show="view.minimize"
                @click="panel.minimizeUpgradeModal()">{{ t('progress.minimize') }}</button>
        <button ref="closeBtn" class="btn-primary btn-block" id="btnUpClose"
                @click="panel.closeUpgradeModal()" v-show="view.close">{{ t('progress.close') }}</button>
      </div>
    </div>
  </dialog>
</template>
