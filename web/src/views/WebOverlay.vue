<script setup lang="ts">
import { usePanel } from "../panelContext";
const panel = usePanel();
</script>

<template>
  <div id="webEntryOverlay" class="web-entry-overlay" role="dialog" aria-modal="true" aria-label="QwenPaw Web" :hidden="!panel.webEntry.open">
    <iframe id="webEntryFrame" title="QwenPaw Web" :src="panel.webEntry.url"></iframe>
  </div>
</template>
