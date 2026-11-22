<script setup lang="ts">
import { computed } from 'vue';
import MirrorPicker from './MirrorPicker.vue';
import VersionPicker from './VersionPicker.vue';
import { usePanel } from '../panelContext';
import { t, type MessageKey } from '../i18n';
import type { InstallActionKind } from '../versionPanel';

const panel = usePanel();
const v = panel.versions.view;
const status = panel.status.view;
const mirror = panel.settings.mirror;
const actionKind = computed(() => panel.versions.currentInstallActionKind());
const actionLabelKeys: Record<InstallActionKind, MessageKey> = {
  install: 'deploy.action.install',
  downgrade: 'deploy.action.downgrade',
  upgrade: 'deploy.action.upgrade',
  reinstall: 'deploy.action.reinstall',
};
const actionLabel = computed(() => !v.selected ? t('deploy.selectTarget') : `${t(actionLabelKeys[actionKind.value])} v${v.selected}`);
const sortHint = computed(() => v.list.every(item => item.published_at) ? t('deploy.sort.allDates') : v.list.some(item => item.published_at) ? t('deploy.sort.partialDates') : t('deploy.sort.noDates'));
</script>

<template>
  <section class="card" id="version" tabindex="-1" data-mobile-panel="overview" :hidden="panel.navigation.hides('overview')" aria-labelledby="versionTitle">
    <div class="card-title">
      <h2 class="card-title-left" id="versionTitle">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 00-2.91-.09zM12 15l-3-3a22 22 0 012-3.95A12.88 12.88 0 0122 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 01-4 2z"/></svg>
        {{ t('deploy.title') }}
      </h2>
      <button type="button" class="btn-outline btn-xs" :disabled="v.loading || mirror.saving" @click="panel.checkOnlineVersions()">{{ t('deploy.checkUpdates') }}</button>
    </div>
    <div class="card-body version-picker">
      <div class="info-row"><span class="info-label">{{ t('deploy.currentKernelVersion') }}</span><strong id="valVersion">{{ status.version }}</strong></div>
      <div id="kernelInstallTip" class="notice notice-info" :hidden="status.data?.installed !== false">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="m12 2 9 5v10l-9 5-9-5V7zM3 7l9 5 9-5m-9 5v10"/></svg>
        <span>{{ t('deploy.uninstalledTip') }}</span>
      </div>
      <button type="button" aria-haspopup="dialog" id="installRunningTip" class="notice notice-warn notice-click" :hidden="!status.busyInstall || panel.progress.view.open" @click="panel.attachRunningInstall()">{{ t('deploy.runningTip') }}</button>
      <MirrorPicker />
      <div id="versionSourceStatus" class="notice notice-danger" role="alert" :hidden="!v.error">{{ v.error }}</div>
      <VersionPicker>
        <small id="versionOrder" class="card-note" :hidden="!v.list.length">{{ sortHint }}</small>
      </VersionPicker>
      <div class="row-inline"><input type="checkbox" id="preinstallBackup" v-model="v.backup"><label for="preinstallBackup" class="field-label">{{ t('deploy.backupBeforeInstall') }}</label></div>
      <div id="downgradeWarnBar" class="notice notice-danger notice-sm" :hidden="!status.reachable || !v.selected || actionKind !== 'downgrade'">{{ t('deploy.downgradeWarn') }}</div>
    </div>
    <div class="card-actions">
      <button type="button" class="btn-primary btn-sm" id="btnUpgradeAction" :class="{ 'btn-danger': v.selected && actionKind === 'downgrade' }" :disabled="!v.fresh || !v.selected || !status.reachable || (status.data?.installed && !v.current) || !!status.recovery || mirror.saving" :title="status.recovery || (!status.reachable ? t('deploy.unreachableTitle') : '')" @click="panel.checkOrUpgrade()">{{ actionLabel }}</button>
      <button type="button" class="btn-danger-ghost btn-sm" id="btnUninstallKernel" :disabled="!v.current" :hidden="!status.data?.installed || !status.reachable || !!status.recovery" @click="panel.uninstallKernel()">{{ t('deploy.uninstallKernel') }}</button>
    </div>
  </section>
</template>
