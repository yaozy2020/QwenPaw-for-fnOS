<script setup lang="ts">
import LogLines from "./LogLines.vue";
import InstallSteps from "./InstallSteps.vue";
import { usePanel } from "../panelContext";
const panel = usePanel();
const view = panel.progress.view;
</script>

<template>
  <div class="modal-overlay" id="upgradeModal" :class="{active: view.open}">
    <div class="modal-box" role="dialog" aria-modal="true" aria-labelledby="upTitle" aria-describedby="upSubTitle" tabindex="-1">
      <div class="modal-header">
        <div class="modal-icon-badge primary">
          <span class="spinner" id="upSpinner" v-show="view.spinner"></span>
          <svg id="upDoneIcon" viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2.5" v-show="view.icon" :style="{color: view.success ? 'var(--success)' : 'var(--danger-text)'}">
            <polyline points="20 6 9 17 4 12"/>
          </svg>
        </div>
        <div class="modal-heading">
          <div class="modal-title" id="upTitle">{{ view.title }}</div>
          <div class="modal-subtitle" id="upSubTitle">{{ view.subtitle }}</div>
        </div>
      </div>
      <ol class="install-steps" id="upSteps" aria-label="安装进度">
        <InstallSteps v-bind="view.steps" />
      </ol>
      <div class="upgrade-terminal-body" id="upTerminal"><LogLines :lines="view.lines" :empty="view.empty" success /></div>
      <div class="modal-actions">
        <!-- 常驻出口:任务执行中也能关掉弹窗(转后台继续跑),
             避免后端失联时页面被永久锁死 -->
        <button class="btn-outline btn-block" id="btnUpMinimize" v-show="view.minimize"
                @click="panel.minimizeUpgradeModal()">转到后台运行</button>
        <button class="btn-primary btn-block" id="btnUpClose"
                @click="panel.closeUpgradeModal()" v-show="view.close">完成并返回控制台</button>
      </div>
    </div>
  </div>
</template>
