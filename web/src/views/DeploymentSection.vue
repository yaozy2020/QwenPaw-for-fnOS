<script setup lang="ts">
import { computed } from 'vue';
import OptionList from './OptionList.vue';
import VersionPicker from './VersionPicker.vue';
import { usePanel } from '../panelContext';
const panel = usePanel();
const v = panel.versions.view;
const status = panel.status.view;
const mirror = panel.settings.mirror;
const action = computed(() => panel.versions.currentInstallAction());
const actionLabel = computed(() => !v.selected ? '请选择目标版本' : ({安装:'安装内核',降级:'降级至',升级:'执行更新至',重新安装:'重新安装'}[action.value] || action.value) + ' v' + v.selected);
const sortHint = computed(() => v.list.every(item => item.published_at) ? '按发布时间从新到旧' : v.list.some(item => item.published_at) ? '按发布时间从新到旧；缺少时间的版本按版本号排在后面' : '此源未提供发布时间，按版本号从高到低');
</script>

<template>
  <section class="card" id="version" tabindex="-1" data-mobile-panel="overview" :hidden="panel.navigation.hides('overview')" aria-labelledby="versionTitle">
    <div class="card-title">
      <h2 class="card-title-left" id="versionTitle">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 00-2.91-.09zM12 15l-3-3a22 22 0 012-3.95A12.88 12.88 0 0122 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 01-4 2z"/></svg>
        部署版本
      </h2>
      <button type="button" class="btn-outline btn-xs" :disabled="v.loading || mirror.saving" @click="panel.checkOnlineVersions()">检查更新</button>
    </div>
    <div class="card-body version-picker">
      <div class="info-row"><span class="info-label">当前内核版本</span><strong id="valVersion">{{ status.version }}</strong></div>
      <div id="kernelInstallTip" class="notice notice-info" :hidden="status.data?.installed !== false">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="m12 2 9 5v10l-9 5-9-5V7zM3 7l9 5 9-5m-9 5v10"/></svg>
        <span>尚未安装内核。首次安装建议选择「fnOS 已验证」版本。</span>
      </div>
      <button type="button" aria-haspopup="dialog" id="installRunningTip" class="notice notice-warn notice-click" :hidden="!status.busyInstall || panel.progress.view.open" @click="panel.attachRunningInstall()">安装任务正在进行中，查看进度</button>
      <div class="field-row">
        <span class="field-label" id="mirrorFieldLabel">下载源</span>
        <div class="field-control">
          <button type="button" class="select-btn" id="mirrorToggleBtn" aria-labelledby="mirrorFieldLabel mirrorLabel" :aria-expanded="mirror.open" :disabled="mirror.saving" aria-controls="mirrorDropdown" @click="panel.toggleMirrorDropdown()"><span id="mirrorLabel">{{ mirror.ready ? panel.settings.mirrors.find(m => m.value === mirror.value)?.label || mirror.value : '查看下载源' }}</span><span class="caret" aria-hidden="true">▾</span></button>
          <div class="history-dropdown float" id="mirrorDropdown" :class="{ show: mirror.open }"><OptionList :options="panel.settings.mirrors" :selected="mirror.value" @pick="panel.settings.pickMirror" /></div>
        </div>
      </div>
      <div id="versionSourceStatus" class="notice notice-danger" role="alert" :hidden="!v.error">{{ v.error }}</div>
      <VersionPicker>
        <small id="versionOrder" class="card-note" :hidden="!v.list.length">{{ sortHint }}</small>
      </VersionPicker>
      <div class="row-inline"><input type="checkbox" id="preinstallBackup" v-model="v.backup"><label for="preinstallBackup" class="field-label">安装前备份已有数据</label></div>
      <div id="downgradeWarnBar" class="notice notice-danger notice-sm" :hidden="!status.reachable || !v.selected || action !== '降级'">目标版本低于当前内核，旧版本可能无法读取新版配置或记忆。</div>
    </div>
    <div class="card-actions">
      <button type="button" class="btn-primary btn-sm" id="btnUpgradeAction" :class="{ 'btn-danger': v.selected && action === '降级' }" :disabled="!v.fresh || !v.selected || !status.reachable || (status.data?.installed && !v.current) || !!status.recovery || mirror.saving" :title="status.recovery || (!status.reachable ? '控制台无法连接监督者，暂不可操作' : '')" @click="panel.checkOrUpgrade()">{{ actionLabel }}</button>
      <button type="button" class="btn-danger-ghost btn-sm" id="btnUninstallKernel" :disabled="!v.current" :hidden="!status.data?.installed || !status.reachable || !!status.recovery" @click="panel.uninstallKernel()">仅卸载内核</button>
    </div>
  </section>
</template>
