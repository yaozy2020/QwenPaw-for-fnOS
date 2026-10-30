<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'

interface LogItem {
  level?: string
  time?: string
  message?: string
}

const toast = useToast()
const showNotification = (message: string, type: 'success' | 'error' | 'warning' | 'info' = 'info') => {
  const color = type === 'error'
    ? 'error'
    : type === 'warning'
      ? 'warning'
      : type === 'success'
        ? 'success'
        : 'neutral'
  const icon = type === 'error'
    ? 'i-lucide-x-circle'
    : type === 'warning'
      ? 'i-lucide-alert-triangle'
      : type === 'success'
        ? 'i-lucide-check-circle'
        : 'i-lucide-info'
  toast.add({ title: message, color, icon })
}

const logs = ref<LogItem[]>([])
const isLoading = ref(false)
const filterText = ref('')

const autoRefresh = ref<boolean>(true)
let refreshTimer: number | null = null
const refreshIntervalMs = 3000

const page = ref(1)
const pageSize = ref(10)
const pageSizeItems = [
  { label: '10 / 页', value: 10 },
  { label: '20 / 页', value: 20 },
  { label: '50 / 页', value: 50 },
  { label: '100 / 页', value: 100 }
]

const filteredLogs = computed(() => {
  const needle = filterText.value.trim().toLowerCase()
  if (!needle) return logs.value
  return logs.value.filter((log) => {
    const msg = (log.message || '').toLowerCase()
    return msg.includes(needle)
  })
})

const pageCount = computed(() => Math.max(1, Math.ceil(filteredLogs.value.length / pageSize.value)))
const pagedLogs = computed(() => {
  const start = (page.value - 1) * pageSize.value
  const end = start + pageSize.value
  return filteredLogs.value.slice(start, end)
})

const clampPage = () => {
  page.value = Math.min(Math.max(1, page.value), pageCount.value)
}

watch([filterText, pageSize], () => {
  page.value = 1
})

watch(pageCount, clampPage, { immediate: true })

const fetchLogs = async (notify = false) => {
  isLoading.value = true
  try {
    const response = await fetch('/cgi/ThirdParty/com.dustinky.qwenpaw/api.cgi?action=logs')
    const result = await response.json() as { success?: boolean; logs?: LogItem[]; message?: string }
    if (result?.success) {
      logs.value = Array.isArray(result.logs) ? result.logs : []
      if (notify) showNotification('日志已刷新', 'success')
      return
    }
    if (notify) showNotification('获取日志失败：' + (result?.message || '未知错误'), 'error')
  } catch (err: unknown) {
    if (notify) {
      const e = err as Error
      showNotification('获取日志失败：' + (e?.message || String(err)), 'error')
    }
  } finally {
    isLoading.value = false
  }
}

const clearLogs = async () => {
  try {
    const response = await fetch('/cgi/ThirdParty/com.dustinky.qwenpaw/api.cgi?action=clear_logs', { method: 'POST' })
    const result = await response.json() as { success?: boolean; message?: string }
    if (result?.success) {
      logs.value = []
      page.value = 1
      showNotification(result?.message || '日志已清空', 'success')
      return
    }
    showNotification('清空日志失败：' + (result?.message || '未知错误'), 'error')
  } catch (err: unknown) {
    const e = err as Error
    showNotification('清空日志失败：' + (e?.message || String(err)), 'error')
  }
}

const scheduleRefresh = () => {
  if (refreshTimer) {
    window.clearTimeout(refreshTimer)
    refreshTimer = null
  }
  if (autoRefresh.value !== true) return

  refreshTimer = window.setTimeout(async () => {
    await fetchLogs()
    scheduleRefresh()
  }, refreshIntervalMs)
}

watch(autoRefresh, () => {
  scheduleRefresh()
}, { immediate: true })

onMounted(async () => {
  await fetchLogs()
})

onUnmounted(() => {
  if (refreshTimer) window.clearTimeout(refreshTimer)
})
</script>

<template>
  <div class="mx-auto space-y-6">
    <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
      <div>
        <h1 class="text-3xl font-bold text-[var(--ui-text)]">
          运行日志
        </h1>
        <p class="text-[var(--ui-text-muted)] mt-2">
          查看 QwenPaw 运行日志与升级日志（最新 500 条）
        </p>
      </div>

      <div class="flex flex-wrap gap-2 w-full md:w-auto">
        <div class="flex items-center gap-2 px-3 py-2 rounded-lg ring-1 ring-primary">
          <span class="text-sm text-[var(--ui-text-muted)]">
            自动刷新
          </span>
          <USwitch
            v-model="autoRefresh"
          />
        </div>
        <UButton
          color="neutral"
          variant="outline"
          icon="i-lucide-rotate-cw"
          :loading="isLoading"
          @click="fetchLogs(true)"
        >
          刷新
        </UButton>
        <UButton
          color="error"
          variant="soft"
          icon="i-lucide-trash-2"
          @click="clearLogs"
        >
          清空
        </UButton>
      </div>
    </div>

    <div class="flex flex-row items-center justify-between gap-3">
      <UInput
        v-model="filterText"
        icon="i-lucide-search"
        placeholder="筛选日志内容..."
        class="w-full md:max-w-md"
      />

      <div class="flex items-center gap-2">
        <span class="text-sm text-[var(--ui-text-muted)] whitespace-nowrap">
          每页
        </span>
        <USelect
          v-model="pageSize"
          class="w-28"
          :items="pageSizeItems"
        />
      </div>
    </div>

    <div class="space-y-3">
      <div
        v-if="pagedLogs.length === 0"
        class="py-10 text-center text-sm text-[var(--ui-text-muted)]"
      >
        暂无日志
      </div>

      <UCard
        v-for="(log, idx) in pagedLogs"
        :key="`${idx}-${log.message ?? ''}`"
        class="rounded-2xl"
        :ui="{ root: 'divide-y-0', body: 'py-3' }"
      >
        <div class="flex items-start gap-3">
          <div class="text-xs text-[var(--ui-text-muted)] font-mono tabular-nums whitespace-nowrap shrink-0">
            {{ log.time || '' }}
          </div>
          <div class="text-sm text-[var(--ui-text)] min-w-0 break-words">
            {{ log.message || '-' }}
          </div>
        </div>
      </UCard>
    </div>

    <div
      v-if="filteredLogs.length > 0"
      class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between"
    >
      <div class="text-sm text-[var(--ui-text-muted)]">
        共 {{ filteredLogs.length }} 条
      </div>
      <UPagination
        v-model:page="page"
        :total="filteredLogs.length"
        :items-per-page="pageSize"
        :disabled="isLoading"
        :show-edges="true"
        :sibling-count="1"
        size="sm"
      />
    </div>
  </div>
</template>