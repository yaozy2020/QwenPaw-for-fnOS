<script setup lang="ts">
import StatusCard from "./StatusCard.vue";
import { computed, ref } from "vue";
import { usePanel } from "../panelContext";
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
        <span class="card-meta" id="statusUptime">{{ status.uptime ? `已运行 ${status.uptime}` : '' }}</span>
        <button type="button" class="btn-outline btn-xs icon-button" id="refreshDashboardBtn" :disabled="refreshing" :aria-busy="refreshing || undefined" @click="refreshDashboard" title="刷新服务状态" aria-label="刷新服务状态">
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
          <div class="status-head"><span class="status-dot" aria-hidden="true"></span><span class="status-text">正在连接服务</span></div>
          <p class="status-description">正在获取运行状态与资源用量…</p>
        </div>
        <div class="stat-grid" aria-label="运行指标">
          <div class="stat"><div class="stat-value">—</div><div class="stat-label">CPU 占用</div></div>
          <div class="stat"><div class="stat-value">—</div><div class="stat-label">内存占用</div></div>
        </div></template>
      </div>
    </div>
    <div class="overview-details" aria-label="当前配置摘要">
      <div><span>进程 PID</span><strong id="overviewPid">{{ status.data?.running ? status.data.pid : '—' }}</strong></div>
      <div><span>当前版本</span><strong id="overviewVersion">{{ status.reachable ? status.version : '—' }}</strong></div>
      <div><span>服务端口</span><strong id="overviewPort">{{ status.reachable ? panel.management.servicePort : '—' }}</strong></div>
      <div><span>登录验证</span><strong id="overviewAuth">{{ status.reachable ? panel.management.auth.label : '—' }}</strong></div>
    </div>
    <div class="overview-installer" id="overviewInstaller" :hidden="!panel.needsInstaller()">
      <div class="installer-summary">
        <div>
          <span>目标版本</span>
          <strong id="overviewInstallVersion">{{ target ? `v${target}${panel.versions.isRecommended() ? ' · 随包适配补丁' : ''}` : '请选择版本' }}</strong>
        </div>
      </div>
      <div class="installer-actions">
        <button type="button" class="btn-primary" id="overviewInstallButton" :disabled="!target || !status.reachable || !!status.recovery || panel.settings.mirror.saving" @click="panel.checkOrUpgrade()">开始安装</button>
      </div>
      <p class="card-note">可在「部署版本」更换目标版本。</p>
    </div>
    <button type="button" id="overviewRunningInstall" class="btn-outline" :hidden="!status.busyInstall || panel.progress.view.open" aria-haspopup="dialog" @click="panel.attachRunningInstall()">安装任务进行中 · 查看进度</button>
    <div class="overview-footer">
      <span class="card-note" id="statusCheckedAt">{{ panel.serviceControl.pending || status.checked }}</span>
      <div class="card-actions">
        <button class="btn-primary btn-sm" id="btnStartService" :disabled="!!panel.serviceControl.pending" @click="panel.controlService('start')" :hidden="!usable || !!status.data?.running">启动服务</button>
        <button class="btn-outline btn-sm" id="btnRestartService" :disabled="!!panel.serviceControl.pending" @click="panel.controlService('restart')" :hidden="!usable || !status.data?.running">重启服务</button>
        <button class="btn-danger-ghost btn-sm" id="btnStopService" :disabled="!!panel.serviceControl.pending" @click="panel.controlService('stop')" :hidden="!usable || !status.data?.running">停止服务</button>
      </div>
    </div>
    <div class="overview-guide" id="overviewGuide" :hidden="!panel.needsInstaller()">
      <h3>首次使用</h3>
      <ol>
        <li>在「部署版本」选择目标版本并确认安装</li>
        <li>等待下载安装与启动检查完成</li>
        <li>进入 Web 对话开始使用</li>
      </ol>
    </div>
  </section>
</template>
