<script setup lang="ts">
import { nextTick, useTemplateRef, watch } from "vue";
import { usePanel } from "../panelContext";
const panel = usePanel();
const backButton = useTemplateRef<HTMLButtonElement>("backButton");
watch(() => panel.webEntry.open, open => {
  if (open) void nextTick(() => backButton.value?.focus());
});
</script>

<template>
  <div id="webEntryOverlay" class="web-entry-overlay" role="dialog" aria-modal="true" aria-label="QwenPaw Web" :hidden="!panel.webEntry.open">
    <button ref="backButton" id="webEntryBack" class="btn-outline" type="button" @click="panel.dismissWeb()">返回控制台</button>
    <iframe id="webEntryFrame" title="QwenPaw Web" :src="panel.webEntry.url"></iframe>
  </div>
</template>
