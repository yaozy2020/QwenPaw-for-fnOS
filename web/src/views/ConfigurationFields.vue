<script setup lang="ts">
import { computed, useTemplateRef } from 'vue';
import NativeLoginDialog from './NativeLoginDialog.vue';
import ResetPasswordDialog from './ResetPasswordDialog.vue';
import type { ManagementState } from '../managementState';
import { localeMode, setLocaleMode, t, type LocaleMode, type MessageKey } from '../i18n';
import { setThemeMode, setUiStyle, themeMode, uiStyle, type ThemeMode, type UiStyle } from '../theme';

const localeOptions: readonly { value: LocaleMode; labelKey: MessageKey }[] = [
  { value: 'system', labelKey: 'config.languageSystem' },
  { value: 'zh-CN', labelKey: 'config.languageZh' },
  { value: 'en', labelKey: 'config.languageEn' },
];

const themeOptions: readonly { value: ThemeMode; labelKey: MessageKey }[] = [
  { value: 'system', labelKey: 'config.themeSystem' },
  { value: 'light', labelKey: 'config.themeLight' },
  { value: 'dark', labelKey: 'config.themeDark' },
];

const uiStyleOptions: readonly { value: UiStyle; labelKey: MessageKey }[] = [
  { value: 'default', labelKey: 'config.uiStyleDefault' },
  { value: 'miuix', labelKey: 'config.uiStyleMiuix' },
];

const props = defineProps<{ state: ManagementState }>();
const emit = defineEmits<{ servicePort: []; proxyPort: []; useNative: [username: string, password: string]; scope: [value: boolean]; reset: [password: string]; migrate: [] }>();
const proxyKnown = computed(() => props.state.proxyPort !== null && props.state.proxyPublic !== null);
const migration = useTemplateRef<InstanceType<typeof NativeLoginDialog>>('migration');
const recovery = useTemplateRef<InstanceType<typeof ResetPasswordDialog>>('recovery');

function changeScope(event: Event) {
  if (event.currentTarget instanceof HTMLInputElement) emit('scope', event.currentTarget.checked);
}
</script>

<template>
  <div class="card-body">
    <div class="info-row">
      <span class="info-label" id="languageFieldLabel">{{ t('config.language') }}</span>
      <div class="log-level-group" id="localeModeGroup" role="group" aria-labelledby="languageFieldLabel">
        <button v-for="option in localeOptions" :key="option.value" type="button" class="btn-outline btn-xs log-level-btn" :data-locale-option="option.value" :aria-pressed="localeMode === option.value" @click="setLocaleMode(option.value)">{{ t(option.labelKey) }}</button>
      </div>
    </div>
    <div class="info-row">
      <span class="info-label" id="themeFieldLabel">{{ t('config.theme') }}</span>
      <div class="log-level-group" id="themeModeGroup" role="group" aria-labelledby="themeFieldLabel">
        <button v-for="option in themeOptions" :key="option.value" type="button" class="btn-outline btn-xs log-level-btn" :data-theme-option="option.value" :aria-pressed="themeMode === option.value" @click="setThemeMode(option.value)">{{ t(option.labelKey) }}</button>
      </div>
    </div>
    <div class="info-row">
      <span class="info-label" id="uiStyleFieldLabel">{{ t('config.uiStyle') }}</span>
      <div class="log-level-group" id="uiStyleGroup" role="group" aria-labelledby="uiStyleFieldLabel">
        <button v-for="option in uiStyleOptions" :key="option.value" type="button" class="btn-outline btn-xs log-level-btn" :data-ui-style-option="option.value" :aria-pressed="uiStyle === option.value" @click="setUiStyle(option.value)">{{ t(option.labelKey) }}</button>
      </div>
    </div>
    <div class="info-row">
      <span class="info-label">{{ t('config.servicePort') }}</span>
      <div class="row-inline">
        <span id="valPort" class="info-val">{{ state.servicePort }}</span>
        <button id="btnChangeServicePort" type="button" class="btn-outline btn-xs" :disabled="!state.serviceReachable || state.saving" :aria-disabled="state.pending || !state.serviceReachable" :title="state.serviceReachable ? '' : t('config.portNotSyncedTitle')" @click="emit('servicePort')">{{ t('config.changePort') }}</button>
      </div>
    </div>
    <div class="info-row">
      <span class="info-label">{{ t('config.proxyPort') }}</span>
      <div class="row-inline">
        <span id="valWebProxyPort" class="info-val">{{ state.proxyPort ?? t('config.autoAssign') }}</span>
        <button type="button" class="btn-outline btn-xs" :disabled="state.saving" :aria-disabled="state.pending" @click="emit('proxyPort')">{{ t('config.changeProxyPort') }}</button>
      </div>
    </div>
    <div class="info-row">
      <span class="info-label">{{ t('config.externalLogin') }}</span>
      <div class="row-inline">
        <span id="proxyPasswordState" class="info-val" aria-live="polite">{{ state.proxyPasswordConfigured === true ? t('config.proxyPasswordLegacy') : state.proxyPasswordConfigured === false ? t('config.proxyPasswordNative') : t('config.proxyPasswordUnknown') }}</span>
        <template v-if="state.proxyPasswordConfigured === true">
          <button type="button" class="btn-outline btn-xs" :disabled="state.pending" @click="migration?.open()">{{ t('config.switchToNative') }}</button>
          <NativeLoginDialog ref="migration" :disabled="state.pending || state.proxyPasswordConfigured !== true" @submit="(u, p) => emit('useNative', u, p)" />
        </template>
      </div>
    </div>
    <label class="toggle-row">
      <input id="webProxyPublic" type="checkbox" :disabled="!proxyKnown || state.saving" :aria-disabled="state.pending || !proxyKnown" :title="proxyKnown ? '' : t('config.proxyUnknownTitle')" :indeterminate.prop="!proxyKnown" :checked="state.proxyDraft" @click="state.pending && $event.preventDefault()" @change="changeScope" />
      <span>{{ t('config.allowExternalProxy') }}</span>
    </label>
    <div class="section-label">{{ t('config.authSection') }}</div>
    <div class="info-row">
      <span class="info-label">{{ t('config.webLoginAuth') }}</span>
      <span id="valAuthStatus" class="info-val is-muted" :style="{ color: `var(--${state.auth.tone})` }">{{ state.auth.label }}</span>
    </div>
    <div class="info-row">
      <span class="info-label">{{ t('config.adminIdentity') }}</span>
      <span id="valAdminStatus" class="info-val is-muted" :style="{ color: `var(--${state.auth.tone})` }">{{ state.auth.admin }}</span>
    </div>
    <p id="valAuthDesc" class="card-note">{{ state.auth.description }}</p>
  </div>
  <div class="card-actions">
    <button type="button" class="btn-outline btn-sm btn-block" :disabled="state.entryLogin !== false || state.saving" :aria-disabled="state.pending" @click="emit('migrate')">{{ state.entryLogin ? t('config.entryMigrated') : t('config.migrateEntry') }}</button>
    <button id="resetAuthBtn" type="button" class="btn-outline btn-sm btn-block" :disabled="state.pending" @click="!state.pending && recovery?.open()">{{ t('config.resetAdminPassword') }}</button>
    <ResetPasswordDialog ref="recovery" :disabled="state.pending" @submit="p => emit('reset', p)" />
  </div>
</template>
