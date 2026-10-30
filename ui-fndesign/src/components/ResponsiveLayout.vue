<script setup lang="ts">
import DesktopSidebar from './DesktopSidebar.vue'
import MobileTabBar from './MobileTabBar.vue'
</script>

<template>
  <div class="h-screen bg-[var(--ui-bg)] overflow-hidden flex flex-col md:flex-row md:pr-2 md:pb-2">
    <DesktopSidebar class="hidden md:flex" />

    <main class="flex-1 overflow-y-auto p-5 pb-21 md:pb-5 bg-[var(--ui-bg-content)] md:rounded-lg">
      <div class="min-h-full">
        <slot />
      </div>
    </main>

    <MobileTabBar class="md:hidden" />
  </div>
</template>
