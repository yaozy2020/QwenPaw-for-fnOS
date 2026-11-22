<script setup lang="ts">
import { ref } from 'vue';
import { closeDialog, showDialog } from './dialogHelpers';
import { t } from '../i18n';

const props = defineProps<{ disabled: boolean }>();
const emit = defineEmits<{ submit: [password: string] }>();
const dialog = ref<HTMLDialogElement>();
const passwordField = ref<HTMLInputElement>();
const confirmation = ref<HTMLInputElement>();

function open() { showDialog(dialog.value); }
function close() { closeDialog(dialog.value); }
function clear() {
  passwordField.value?.form?.reset();
  passwordField.value?.setCustomValidity('');
  confirmation.value?.setCustomValidity('');
}
function submit() {
  const password = passwordField.value, repeat = confirmation.value;
  if (!password || !repeat || props.disabled) return;
  const invalid = !password.value.trim() ? password : password.value !== repeat.value ? repeat : null;
  if (invalid) {
    invalid.setCustomValidity(invalid === password ? t('resetPwd.blankError') : t('resetPwd.mismatchError'));
    invalid.reportValidity();
    return;
  }
  close();
  emit('submit', password.value);
}
defineExpose({ open });
</script>

<template>
  <dialog ref="dialog" class="about-dialog native-login-dialog" aria-labelledby="recoveryTitle" aria-describedby="recoveryHint" @close="clear">
    <form method="dialog" @submit.prevent="submit">
      <h3 id="recoveryTitle" class="modal-title">{{ t('resetPwd.title') }}</h3>
      <p id="recoveryHint" class="modal-msg">{{ t('resetPwd.hint') }}</p>
      <label for="newAdminPassword" class="modal-field modal-label">{{ t('resetPwd.newPassword') }}<input id="newAdminPassword" ref="passwordField" class="modal-input" name="password" type="password" autocomplete="new-password" required autofocus @input="passwordField?.setCustomValidity(''); confirmation?.setCustomValidity('')"></label>
      <label for="confirmAdminPassword" class="modal-field modal-label">{{ t('resetPwd.confirmPassword') }}<input id="confirmAdminPassword" ref="confirmation" class="modal-input" name="confirmation" type="password" autocomplete="new-password" required @input="confirmation?.setCustomValidity('')"></label>
      <div class="modal-actions">
        <button type="button" class="btn-outline" @click="close">{{ t('common.cancel') }}</button>
        <button type="submit" class="btn btn-primary">{{ t('resetPwd.submit') }}</button>
      </div>
    </form>
  </dialog>
</template>
