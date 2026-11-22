<script setup lang="ts">
import type { AuthorizedDirsState } from '../authorizedDirsPanel';
import type { ManagementState } from '../managementState';
import { t } from '../i18n';
defineProps<{ state: AuthorizedDirsState; service: ManagementState }>();
const emit = defineEmits<{ refresh: []; add: []; open: [path: string]; reauthorize: [path: string]; remove: [path: string]; backup: [] }>();
</script>

<template>
  <div id="authorized-dirs">
    <div class="info-row">
      <span id="authorizedDirsTitle" class="info-label">{{ t('authz.workspaceDirs') }}</span>
      <div class="row-inline">
        <button id="refreshAuthorizedDirsBtn" type="button" class="btn-outline btn-xs" :disabled="state.busy" @click="emit('refresh')">{{ t('authz.refresh') }}</button>
        <button id="addAuthorizedDirBtn" type="button" class="btn-primary btn-xs" :disabled="state.busy" @click="emit('add')">{{ t('authz.addDir') }}</button>
      </div>
    </div>
    <p class="section-subtitle">{{ t('authz.subtitle') }}</p>
    <p id="authorizedDirsStatus" class="card-note" role="status" aria-live="polite" :data-error="state.error">{{ state.message }}</p>
    <div id="authorizedDirsList" aria-live="polite">
      <template v-if="state.listError">{{ state.listError }}</template>
      <span v-else-if="state.entries === null" class="log-empty" role="status">{{ t('authz.loading') }}</span>
      <span v-else-if="!state.entries.length" class="log-empty">{{ t('authz.empty') }}</span>
      <div v-for="entry in state.listError ? [] : state.entries" :key="entry.path" class="info-row">
        <span class="authorized-dir-value" :title="entry.path">{{ entry.label ? `${entry.label} · ${entry.path}` : entry.path }}</span>
        <div class="authorized-dir-actions">
          <button v-if="state.canOpen" type="button" class="btn-outline btn-xs" :disabled="state.busy" @click="emit('open', entry.path)">{{ t('authz.openDir') }}</button>
          <button type="button" class="btn-outline btn-xs" :disabled="state.busy" @click="emit('reauthorize', entry.path)">{{ t('authz.reauthorize') }}</button>
          <button type="button" class="btn-danger-ghost btn-xs" :disabled="state.busy && state.confirmingRemove !== entry.path" :aria-disabled="state.busy" @click="emit('remove', entry.path)">{{ t('authz.removeBind') }}</button>
        </div>
      </div>
    </div>
  </div>
  <div class="section-label">{{ t('authz.backupSection') }}</div>
  <div class="info-row">
    <div>
      <span class="info-label">{{ t('authz.backupTitle') }}</span>
      <p class="card-note">{{ t('authz.backupDesc') }}</p>
    </div>
    <button id="openBackupBtn" type="button" class="btn-outline btn-xs" :disabled="!service.backupAvailable" :title="service.backupAvailable ? t('authz.openBackupReadyTitle') : t('authz.openBackupDisabledTitle')" @click="emit('backup')">{{ t('authz.openBackup') }}</button>
  </div>
  <details class="card-note">
    <summary>{{ t('authz.offlineRecoverySummary') }}</summary>
    <p>{{ t('authz.offlineRecoveryP1') }}<code>preinstall-data.tar</code>{{ t('authz.offlineRecoveryP1End') }}</p>
    <p>{{ t('authz.offlineRecoveryP2a') }}<code>.qwenpaw</code>{{ t('authz.offlineRecoveryP2b') }}<code>.qwenpaw.secret</code>{{ t('authz.offlineRecoveryP2c') }}</p>
    <p>{{ t('authz.offlineRecoveryP3') }}</p>
  </details>
</template>
