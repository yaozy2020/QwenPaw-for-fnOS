<script setup lang="ts">
import { nextTick, ref, watch } from 'vue';
import { closeDialog, showDialog } from './dialogHelpers';
import { usePanel } from '../panelContext';
import { t } from '../i18n';
const { prompt } = usePanel();
const dialog = ref<HTMLDialogElement>();
const input = ref<HTMLInputElement>();
const confirmButton = ref<HTMLButtonElement>();
const cancelButton = ref<HTMLButtonElement>();
const invalid = ref(false);
const value = ref('');
let opened = 0;
watch(() => prompt.state.options, async options => {
  invalid.value = false;
  value.value = options?.input?.value || '';
  const el = dialog.value;
  if (!options) {
    closeDialog(el);
    return;
  }
  await nextTick();
  if (prompt.state.options !== options) return;
  opened = Date.now();
  showDialog(el);
  if (options.input) { input.value?.focus(); input.value?.select(); }
  else (options.type === 'danger' ? cancelButton : confirmButton).value?.focus();
}, {flush:'post'});
function validate() {
  const spec = prompt.state.options?.input;
  const result = spec?.type === 'password' ? value.value : value.value.trim();
  invalid.value = !!spec && (input.value?.validity.valid === false || !!result && spec.validate?.(result) === false);
  return result;
}
function submit() {
  if (!prompt.state.options?.input) { prompt.finish(true); return; }
  const result = validate();
  if (invalid.value) { input.value?.focus(); return; }
  prompt.finish(result || null);
}
function cancel() { prompt.finish(prompt.state.options?.input ? null : false); }
</script>

<template>
  <dialog ref="dialog" class="modal-overlay" :class="{active: !!prompt.state.options}" id="customModal" aria-labelledby="modalTitle" aria-describedby="modalMessage" @cancel.prevent="cancel" @click.self="Date.now() - opened > 250 && cancel()">
    <form class="modal-box" novalidate @submit.prevent="submit">
      <div class="modal-header"><div id="modalBadge" class="modal-icon-badge" :class="prompt.state.options?.type || 'primary'" aria-hidden="true">!</div><div id="modalTitle" class="modal-title">{{ prompt.state.options?.title || t('common.promptTitle') }}</div></div>
      <div id="modalMessage" class="modal-msg">{{ prompt.state.options?.message }}</div>
      <div class="modal-field" id="modalFieldWrap" :hidden="!prompt.state.options?.input">
        <label class="modal-label" id="modalFieldLabel" for="modalInput">{{ prompt.state.options?.input?.label || t('common.inputLabel') }}</label>
        <input ref="input" id="modalInput" class="modal-input" v-model="value" :type="prompt.state.options?.input?.type || 'text'" :pattern="prompt.state.options?.input?.pattern" :required="!!prompt.state.options?.input?.invalidMessage" :placeholder="prompt.state.options?.input?.placeholder" :inputmode="prompt.state.options?.input?.inputMode" :autocomplete="prompt.state.options?.input?.type === 'password' ? 'new-password' : 'off'" :aria-invalid="invalid" :aria-describedby="invalid ? 'modalMessage modalFieldError' : 'modalMessage'" @input="invalid && validate()" />
        <span id="modalFieldError" role="alert" :hidden="!invalid">{{ invalid ? prompt.state.options?.input?.invalidMessage || t('common.invalidInput') : '' }}</span>
      </div>
      <div class="modal-actions">
        <button ref="cancelButton" type="button" class="btn-outline" id="modalCancelBtn" @click="cancel">{{ prompt.state.options?.cancelText || t('common.cancel') }}</button>
        <button ref="confirmButton" type="submit" class="btn" :class="prompt.state.options?.type === 'danger' ? 'btn-danger' : 'btn-primary'" id="modalConfirmBtn">{{ prompt.state.options?.confirmText || t('common.confirm') }}</button>
      </div>
    </form>
  </dialog>
</template>
