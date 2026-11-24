<script setup lang="ts">
import { useTemplateRef } from 'vue';
import { closeDialog, showDialog } from './dialogHelpers';
import { t, type MessageKey } from '../i18n';
declare const __APP_VERSION__: string;
const appVersion = __APP_VERSION__;
const dialog = useTemplateRef<HTMLDialogElement>('dialog');
const faqItems: readonly { q: MessageKey; a: MessageKey }[] = [
  { q: 'about.faq1.q', a: 'about.faq1.a' },
  { q: 'about.faq2.q', a: 'about.faq2.a' },
  { q: 'about.faq3.q', a: 'about.faq3.a' },
  { q: 'about.faq4.q', a: 'about.faq4.a' },
  { q: 'about.faq5.q', a: 'about.faq5.a' },
  { q: 'about.faq6.q', a: 'about.faq6.a' },
  { q: 'about.faq7.q', a: 'about.faq7.a' },
];
function open() { showDialog(dialog.value); }
function close() { closeDialog(dialog.value); }
defineExpose({ open });
</script>

<template>
  <dialog ref="dialog" id="aboutDialog" class="about-dialog" aria-labelledby="aboutTitle">
    <div class="about-header">
      <div class="about-title-row">
        <h2 id="aboutTitle">{{ t('about.title') }}</h2>
        <span id="aboutVersion" class="about-version">{{ t('about.versionBadge', { version: appVersion }) }}</span>
      </div>
      <p>{{ t('about.desc') }}</p>
    </div>
    <div class="about-links" :aria-label="t('about.linksAria')">
      <a href="https://github.com/agentscope-ai/QwenPaw" target="_blank" rel="noopener noreferrer">
        <strong>{{ t('about.upstreamTitle') }}</strong><span>github.com/agentscope-ai/QwenPaw</span>
      </a>
      <a href="https://github.com/yaozy2020/QwenPaw-for-fnOS" target="_blank" rel="noopener noreferrer">
        <strong>{{ t('about.wrapperTitle') }}</strong><span>github.com/yaozy2020/QwenPaw-for-fnOS</span>
      </a>
    </div>
    <section class="faq" aria-labelledby="faqTitle">
      <h3 id="faqTitle">{{ t('about.faqTitle') }}</h3>
      <details v-for="item in faqItems" :key="item.q">
        <summary>{{ t(item.q) }}</summary>
        <p>{{ t(item.a) }}</p>
      </details>
    </section>
    <form method="dialog" class="about-actions" @submit="close">
      <button type="submit" class="btn-primary" autofocus>{{ t('common.close') }}</button>
    </form>
  </dialog>
</template>
