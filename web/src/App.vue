<script setup lang="ts">
import PanelHeader from "./views/PanelHeader.vue";
import SectionNav from "./views/SectionNav.vue";
import OverviewSection from "./views/OverviewSection.vue";
import DeploymentSection from "./views/DeploymentSection.vue";
import ManagementSections from "./views/ManagementSections.vue";
import LogsSection from "./views/LogsSection.vue";
import ConfirmPrompt from "./views/ConfirmPrompt.vue";
import InstallDialog from "./views/InstallDialog.vue";
import AboutDialog from "./views/AboutDialog.vue";
import WebOverlay from "./views/WebOverlay.vue";
import { useTemplateRef } from 'vue';
import { usePanel } from "./panelContext";
const panel = usePanel();
const about = useTemplateRef<InstanceType<typeof AboutDialog>>('about');
</script>

<template>
  <PanelHeader @about="about?.open()" />
  <SectionNav />
  <main class="main-content">
    <div class="grid">
      <OverviewSection />
      <DeploymentSection />
      <ManagementSections />
    </div>
    <LogsSection />
  </main>
  <div id="toast" role="status" aria-live="polite" :class="{ show: panel.toast.visible }">{{ panel.toast.message }}</div>
  <ConfirmPrompt />
  <InstallDialog />
  <AboutDialog ref="about" />
  <WebOverlay />
</template>
