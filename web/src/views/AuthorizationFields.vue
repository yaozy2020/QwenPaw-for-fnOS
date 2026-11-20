<script setup lang="ts">
import type { AuthorizedDirsState } from '../authorizedDirsPanel';
import type { ManagementState } from '../managementState';
defineProps<{ state: AuthorizedDirsState; service: ManagementState }>();
const emit = defineEmits<{ refresh: []; add: []; open: [path: string]; reauthorize: [path: string]; remove: [path: string]; backup: [] }>();
</script>

<template>
  <div id="authorized-dirs">
    <div class="info-row">
      <span id="authorizedDirsTitle" class="info-label">工作区目录</span>
      <div class="row-inline">
        <button id="refreshAuthorizedDirsBtn" type="button" class="btn-outline btn-xs" :disabled="state.busy" @click="emit('refresh')">刷新</button>
        <button id="addAuthorizedDirBtn" type="button" class="btn-primary btn-xs" :disabled="state.busy" @click="emit('add')">选择飞牛目录</button>
      </div>
    </div>
    <p class="section-subtitle">通过飞牛授权添加项目目录；移除绑定不会删除文件。</p>
    <p id="authorizedDirsStatus" class="card-note" role="status" aria-live="polite" :data-error="state.error">{{ state.message }}</p>
    <div id="authorizedDirsList" aria-live="polite">
      <template v-if="state.listError">{{ state.listError }}</template>
      <span v-else-if="state.entries === null" class="log-empty" role="status">正在读取工作区目录…</span>
      <span v-else-if="!state.entries.length" class="log-empty">尚未绑定项目目录。选择飞牛目录后，可在对话和文件区中使用。</span>
      <div v-for="entry in state.listError ? [] : state.entries" :key="entry.path" class="info-row">
        <span class="authorized-dir-value" :title="entry.path">{{ entry.label ? `${entry.label} · ${entry.path}` : entry.path }}</span>
        <div class="authorized-dir-actions">
          <button v-if="state.canOpen" type="button" class="btn-outline btn-xs" :disabled="state.busy" @click="emit('open', entry.path)">打开目录</button>
          <button type="button" class="btn-outline btn-xs" :disabled="state.busy" @click="emit('reauthorize', entry.path)">重新授权</button>
          <button type="button" class="btn-danger-ghost btn-xs" :disabled="state.busy && state.confirmingRemove !== entry.path" :aria-disabled="state.busy" @click="emit('remove', entry.path)">移除绑定</button>
        </div>
      </div>
    </div>
  </div>
  <div class="section-label">数据备份</div>
  <div class="info-row">
    <div>
      <span class="info-label">QwenPaw 备份</span>
      <p class="card-note">使用上游备份页面创建、导入和恢复；备份可能包含敏感凭证。</p>
    </div>
    <button id="openBackupBtn" type="button" class="btn-outline btn-xs" :disabled="!service.backupAvailable" :title="service.backupAvailable ? '打开 QwenPaw 上游备份管理' : '启动 QwenPaw 后可使用备份管理'" @click="emit('backup')">打开备份管理</button>
  </div>
  <details class="card-note">
    <summary>安装失败，如何离线恢复？</summary>
    <p>安装前可在「概览 → 部署版本」勾选备份。已有数据会保存到应用私有 var 目录的 <code>preinstall-data.tar</code>，不覆盖旧备份。空安装不会生成文件。</p>
    <p>用 NAS 管理员权限将 tar 复制到应用目录外的私有位置。停用应用，解到一个空目录，核对 <code>.qwenpaw</code> 和原有的 <code>.qwenpaw.secret</code>，先另存现有目录，再按原属主权限还原。不要把它导入上游 ZIP 备份页。</p>
    <p>备份可能含密钥，不要公开分享。授权的外部目录、外壳设置和运行环境不在其中；彻底卸载前必须移出备份。</p>
  </details>
</template>
