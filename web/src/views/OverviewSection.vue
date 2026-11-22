<script setup lang="ts">
import StatusCard from "./StatusCard.vue";
import { computed, ref } from "vue";
import { usePanel } from "../panelContext";
import { t } from "../i18n";
const panel = usePanel();
const status = panel.status.view;
const usable = computed(() => !!status.data?.installed && status.reachable && !status.recovery);
const target = computed(() => panel.versions.selectedVersion());
const refreshing = ref(false);
async function refreshDashboard() {
  if (refreshing.value) return;
  refreshing.value = true;
  try { await panel.status.refresh(); }
  finally { refreshing.value = false; }
}
</script>

<template>
  <section class="card" id="overview" tabindex="-1" data-mobile-panel="overview" :hidden="panel.navigation.hides('overview')" aria-labelledby="overviewTitle">
    <div class="card-title">
      <h2 class="card-title-left" id="overviewTitle">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/>
        </svg>
        QwenPaw
      </h2>
      <div class="overview-tools">
        <span class="card-meta" id="statusUptime">{{ status.uptime ? t('overview.uptimePrefix', { uptime: status.uptime }) : '' }}</span>
        <button type="button" class="btn-outline btn-xs icon-button" id="refreshDashboardBtn" :disabled="refreshing" :aria-busy="refreshing || undefined" @click="refreshDashboard" :title="t('overview.refreshTitle')" :aria-label="t('overview.refreshTitle')">
          <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <path d="M23 4v6h-6M1 20v-6h6"/>
            <path d="M3.51 9a9 9 0 0114.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0020.49 15"/>
          </svg>
        </button>
      </div>
    </div>
    <div class="card-body">
      <div id="statusCardMount">
        <StatusCard v-if="status.data" :data="status.data" :uptime="status.uptime" />
        <template v-else><div class="status-summary">
          <div class="status-head"><span class="status-dot" aria-hidden="true"></span><span class="status-text">{{ t('overview.connecting') }}</span></div>
          <p class="status-description">{{ t('overview.connectingDesc') }}</p>
        </div>
        <div class="stat-grid" :aria-label="t('overview.metricsAria')">
          <div class="stat"><div class="stat-value">—</div><div class="stat-label">{{ t('overview.cpu') }}</div></div>
          <div class="stat"><div class="stat-value">—</div><div class="stat-label">{{ t('overview.memory') }}</div></div>
        </div></template>
      </div>
    </div>
    <div class="overview-details" :aria-label="t('overview.detailsAria')">
      <div><span>{{ t('overview.pid') }}</span><strong id="overviewPid">{{ status.data?.running ? status.data.pid : '—' }}</strong></div>
      <div><span>{{ t('overview.currentVersion') }}</span><strong id="overviewVersion">{{ status.reachable ? status.version : '—' }}</strong></div>
      <div><span>{{ t('overview.servicePort') }}</span><strong id="overviewPort">{{ status.reachable ? panel.management.servicePort : '—' }}</strong></div>
      <div><span>{{ t('overview.authStatus') }}</span><strong id="overviewAuth">{{ status.reachable ? panel.management.auth.label : '—' }}</strong></div>
    </div>
    <div class="overview-installer" id="overviewInstaller" :hidden="!panel.needsInstaller()">
      <div class="installer-summary">
        <div>
          <span>{{ t('overview.targetVersion') }}</span>
          <strong id="overviewInstallVersion">{{ target ? `v${target}${panel.versions.isRecommended() ? ` · ${t('overview.bundledPatch')}` : ''}` : t('overview.selectVersion') }}</strong>
        </div>
      </div>
      <div class="installer-actions">
        <button type="button" class="btn-primary" id="overviewInstallButton" :disabled="!target || !status.reachable || !!status.recovery || panel.settings.mirror.saving" @click="panel.checkOrUpgrade()">{{ t('overview.startInstall') }}</button>
      </div>
      <p class="card-note">{{ t('overview.changeTargetNote') }}</p>
    </div>
    <button type="button" id="overviewRunningInstall" class="btn-outline" :hidden="!status.busyInstall || panel.progress.view.open" aria-haspopup="dialog" @click="panel.attachRunningInstall()">{{ t('overview.runningInstall') }}</button>
    <div class="overview-footer">
      <span class="card-note" id="statusCheckedAt">{{ panel.serviceControl.pending || status.checked }}</span>
      <div class="card-actions">
        <button class="btn-primary btn-sm" id="btnStartService" :disabled="!!panel.serviceControl.pending" @click="panel.controlService('start')" :hidden="!usable || !!status.data?.running">{{ t('overview.startService') }}</button>
        <button class="btn-outline btn-sm" id="btnRestartService" :disabled="!!panel.serviceControl.pending" @click="panel.controlService('restart')" :hidden="!usable || !status.data?.running">{{ t('overview.restartService') }}</button>
        <button class="btn-danger-ghost btn-sm" id="btnStopService" :disabled="!!panel.serviceControl.pending" @click="panel.controlService('stop')" :hidden="!usable || !status.data?.running">{{ t('overview.stopService') }}</button>
      </div>
    </div>
    <div class="overview-guide" id="overviewGuide" :hidden="!panel.needsInstaller()">
      <h3>{{ t('overview.firstUse') }}</h3>
      <ol>
        <li>{{ t('overview.guideStep1') }}</li>
        <li>{{ t('overview.guideStep2') }}</li>
        <li>{{ t('overview.guideStep3') }}</li>
      </ol>
    </div>
  </section>
</template>
