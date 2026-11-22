<script setup lang="ts">
import { ref } from 'vue';
import { closeDialog, showDialog } from './dialogHelpers';
import { t } from '../i18n';

const props = defineProps<{ disabled: boolean }>();
const emit = defineEmits<{ submit: [username: string, password: string] }>();
const dialog = ref<HTMLDialogElement>();
const form = ref<HTMLFormElement>();

function open() { showDialog(dialog.value); }
function close() { closeDialog(dialog.value); }
function submit() {
  if (!form.value || props.disabled) return;
  const fields = new FormData(form.value);
  close();
  form.value.reset();
  emit('submit', String(fields.get('username')), String(fields.get('password')));
}
defineExpose({ open });
</script>

<template>
  <dialog ref="dialog" class="about-dialog native-login-dialog" aria-labelledby="nativeLoginTitle" aria-describedby="nativeLoginHint" @close="form?.reset()">
    <form ref="form" method="dialog" @submit.prevent="submit">
      <h3 id="nativeLoginTitle" class="modal-title">{{ t('nativeLogin.title') }}</h3>
      <p id="nativeLoginHint" class="modal-msg">{{ t('nativeLogin.hint') }}</p>
      <label class="modal-field modal-label">{{ t('nativeLogin.username') }}<input class="modal-input" name="username" autocomplete="username" required></label>
      <label class="modal-field modal-label">{{ t('nativeLogin.password') }}<input class="modal-input" name="password" type="password" autocomplete="current-password" required></label>
      <div class="modal-actions">
        <button type="button" class="btn-outline" autofocus @click="close">{{ t('common.cancel') }}</button>
        <button type="submit" class="btn btn-primary">{{ t('nativeLogin.submit') }}</button>
      </div>
    </form>
  </dialog>
</template>
