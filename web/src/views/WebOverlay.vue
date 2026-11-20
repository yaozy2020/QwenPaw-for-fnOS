<script setup lang="ts">
import { usePanel } from "../panelContext";
const panel = usePanel();
</script>

<template>
  <div id="webEntryOverlay" class="web-entry-overlay" role="dialog" aria-modal="true" aria-label="QwenPaw Web" hidden>
    <button id="webEntryBack" class="btn-outline" type="button" @click="panel.dismissWeb()">返回控制台</button>
    <iframe id="webEntryFrame" title="QwenPaw Web" src="about:blank"></iframe>
  </div>
</template>
