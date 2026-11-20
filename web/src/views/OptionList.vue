<script setup lang="ts">
export interface ListOption { value: string; label: string; tag?: string; kind?: string }
defineProps<{ options: ListOption[]; selected?: string }>();
const emit = defineEmits<{ pick: [value: string] }>();
</script>
<template>
  <button v-for="item in options" :key="item.value" type="button" class="history-item" :aria-pressed="selected === undefined ? undefined : selected === item.value" @click="emit('pick', item.value)">
    <span :class="{ 'history-version': item.tag }">{{ item.label }}</span>
    <span v-if="item.tag" class="ver-tag-pill" :class="item.kind">{{ item.tag }}</span>
    <span v-else-if="selected === item.value" aria-hidden="true">✓</span>
  </button>
</template>
