<script setup lang="ts">
import { useTheme } from '@/composables/useTheme'

useTheme()
</script>

<template>
  <Suspense>
    <UApp>
      <ResponsiveLayout>
        <RouterView />
      </ResponsiveLayout>
    </UApp>
  </Suspense>
</template>
