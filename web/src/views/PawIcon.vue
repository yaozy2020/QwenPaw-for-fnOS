<script setup lang="ts">
withDefaults(defineProps<{ size?: number }>(), { size: 26 });
</script>

<template>
  <svg viewBox="0 0 24 24" :width="size" :height="size" fill="currentColor" aria-hidden="true">
    <circle cx="8.5" cy="5.5" r="2.5"/>
    <circle cx="15.5" cy="5.5" r="2.5"/>
    <circle cx="4.5" cy="11.5" r="2.5"/>
    <circle cx="19.5" cy="11.5" r="2.5"/>
    <path d="M12 11c-3.5 0-6 2.5-6 6.5 0 2.2 1.8 4 4 4 1.2 0 2.4-.6 3-1.6.6 1 1.8 1.6 3 1.6 2.2 0 4-1.8 4-4 0-4-2.5-6.5-6-6.5z"/>
  </svg>
</template>
