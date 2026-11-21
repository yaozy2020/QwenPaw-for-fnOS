<script setup lang="ts">
import { computed, ref } from 'vue';
import type { ManagementState } from '../managementState';
const props = defineProps<{ state: ManagementState }>();
const emit = defineEmits<{ servicePort: []; proxyPort: []; useNative: [username: string, password: string]; scope: [value: boolean]; reset: [password: string]; migrate: [] }>();
const proxyKnown = computed(() => props.state.proxyPort !== null && props.state.proxyPublic !== null);
const migration = ref<HTMLDialogElement>();
const form = ref<HTMLFormElement>();
function openDialog(el?: HTMLDialogElement) {
  if (!el || el.open) return;
  if (typeof el.showModal === 'function') el.showModal();
  else el.setAttribute('open', '');
}
function closeDialog(el?: HTMLDialogElement) {
  if (!el) return;
  if (typeof el.close === 'function') el.close();
  else el.removeAttribute('open');
}
function submitNative() {
  if (!form.value || props.state.pending || props.state.proxyPasswordConfigured !== true) return;
  const fields = new FormData(form.value);
  closeDialog(migration.value);
  form.value.reset();
  emit('useNative', String(fields.get('username')), String(fields.get('password')));
}
const recovery = ref<HTMLDialogElement>();
const passwordField = ref<HTMLInputElement>();
const confirmation = ref<HTMLInputElement>();
function submitReset() {
  const password = passwordField.value, repeat = confirmation.value;
  if (!password || !repeat || props.state.pending) return;
  const invalid = !password.value.trim() ? password : password.value !== repeat.value ? repeat : null;
  if (invalid) {
    invalid.setCustomValidity(invalid === password ? '新密码不能全为空格' : '两次输入不一致，请重新确认');
    invalid.reportValidity();
    return;
  }
  closeDialog(recovery.value);
  emit('reset', password.value);
}
function clearRecovery() {
  passwordField.value?.form?.reset();
  passwordField.value?.setCustomValidity('');
  confirmation.value?.setCustomValidity('');
}
function changeScope(event: Event) {
  if (event.currentTarget instanceof HTMLInputElement) emit('scope', event.currentTarget.checked);
}
</script>

<template>
  <div class="card-body">
    <div class="info-row">
      <span class="info-label">服务端口</span>
      <div class="row-inline">
        <span id="valPort" class="info-val">{{ state.servicePort }}</span>
        <button id="btnChangeServicePort" type="button" class="btn-outline btn-xs" :disabled="!state.serviceReachable || state.saving" :aria-disabled="state.pending || !state.serviceReachable" :title="state.serviceReachable ? '' : '服务状态尚未同步，请先刷新'" @click="emit('servicePort')">修改端口</button>
      </div>
    </div>
    <div class="info-row">
      <span class="info-label">独立 Web 反代端口</span>
      <div class="row-inline">
        <span id="valWebProxyPort" class="info-val">{{ state.proxyPort ?? '自动分配' }}</span>
        <button type="button" class="btn-outline btn-xs" :disabled="state.saving" :aria-disabled="state.pending" @click="emit('proxyPort')">修改反代端口</button>
      </div>
    </div>
    <div class="info-row">
      <span class="info-label">外部 Web 登录</span>
      <div class="row-inline">
        <span id="proxyPasswordState" class="info-val" aria-live="polite">{{ state.proxyPasswordConfigured === true ? '旧反代密码保护中' : state.proxyPasswordConfigured === false ? 'QwenPaw 原生账号' : '无法确认' }}</span>
        <template v-if="state.proxyPasswordConfigured === true">
          <button type="button" class="btn-outline btn-xs" :disabled="state.pending" @click="openDialog(migration)">切换为原生登录</button>
          <dialog ref="migration" class="about-dialog native-login-dialog" aria-labelledby="nativeLoginTitle" aria-describedby="nativeLoginHint" @close="form?.reset()">
            <form ref="form" method="dialog" @submit.prevent="submitNative">
              <h3 id="nativeLoginTitle" class="modal-title">切换为原生登录</h3>
              <p id="nativeLoginHint" class="modal-msg">请输入 QwenPaw 原账号，不是飞牛账号。验证成功才停用旧反代密码，已登录反代的设备需要重新登录。</p>
              <label class="modal-field modal-label">QwenPaw 用户名<input class="modal-input" name="username" autocomplete="username" required></label>
              <label class="modal-field modal-label">QwenPaw 密码<input class="modal-input" name="password" type="password" autocomplete="current-password" required></label>
              <div class="modal-actions"><button type="button" class="btn-outline" autofocus @click="closeDialog(migration)">取消</button><button type="submit" class="btn btn-primary">验证并切换</button></div>
            </form>
          </dialog>
        </template>
      </div>
    </div>
    <label class="toggle-row">
      <input id="webProxyPublic" type="checkbox" :disabled="!proxyKnown || state.saving" :aria-disabled="state.pending || !proxyKnown" :title="proxyKnown ? '' : '无法确认反代端口或访问范围，请重试'" :indeterminate.prop="!proxyKnown" :checked="state.proxyDraft" @click="state.pending && $event.preventDefault()" @change="changeScope" />
      <span>允许从其他设备访问独立 Web 反代（默认仅本机）</span>
    </label>
    <div class="section-label">访问鉴权</div>
    <div class="info-row">
      <span class="info-label">Web 登录验证</span>
      <span id="valAuthStatus" class="info-val is-muted" :style="{ color: `var(--${state.auth.tone})` }">{{ state.auth.label }}</span>
    </div>
    <div class="info-row">
      <span class="info-label">管理员身份</span>
      <span id="valAdminStatus" class="info-val is-muted" :style="{ color: `var(--${state.auth.tone})` }">{{ state.auth.admin }}</span>
    </div>
    <p id="valAuthDesc" class="card-note">{{ state.auth.description }}</p>
  </div>
  <div class="card-actions">
    <button type="button" class="btn-outline btn-sm btn-block" :disabled="state.entryLogin !== false || state.saving" :aria-disabled="state.pending" @click="emit('migrate')">{{ state.entryLogin ? '已切换为入口登录' : '迁移旧账号到入口登录' }}</button>
    <button id="resetAuthBtn" type="button" class="btn-outline btn-sm btn-block" :disabled="state.pending" @click="!state.pending && openDialog(recovery)">重置 QwenPaw 管理员密码</button>
    <dialog ref="recovery" class="about-dialog native-login-dialog" aria-labelledby="recoveryTitle" aria-describedby="recoveryHint" @close="clearRecovery">
      <form method="dialog" @submit.prevent="submitReset">
        <h3 id="recoveryTitle" class="modal-title">重置 QwenPaw 管理员密码</h3>
        <p id="recoveryHint" class="modal-msg">不需要旧密码。保存后请用原用户名和新密码登录，已有的 QwenPaw 原生登录会话将失效。飞牛账号和聊天记录不受影响。</p>
        <label for="newAdminPassword" class="modal-field modal-label">新密码<input id="newAdminPassword" ref="passwordField" class="modal-input" name="password" type="password" autocomplete="new-password" required autofocus @input="passwordField?.setCustomValidity(''); confirmation?.setCustomValidity('')"></label>
        <label for="confirmAdminPassword" class="modal-field modal-label">确认新密码<input id="confirmAdminPassword" ref="confirmation" class="modal-input" name="confirmation" type="password" autocomplete="new-password" required @input="confirmation?.setCustomValidity('')"></label>
        <div class="modal-actions"><button type="button" class="btn-outline" @click="closeDialog(recovery)">取消</button><button type="submit" class="btn btn-primary">保存新密码</button></div>
      </form>
    </dialog>
  </div>
</template>
