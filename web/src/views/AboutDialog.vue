<script setup lang="ts">
import { useTemplateRef } from 'vue';
import { closeDialog, showDialog } from './dialogHelpers';
import { t } from '../i18n';
const dialog = useTemplateRef<HTMLDialogElement>('dialog');
function open() { showDialog(dialog.value); }
function close() { closeDialog(dialog.value); }
defineExpose({ open });
</script>

<template>
  <dialog ref="dialog" id="aboutDialog" class="about-dialog" aria-labelledby="aboutTitle">
    <div class="about-header">
      <h2 id="aboutTitle">{{ t('about.title') }}</h2>
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
      <details>
        <summary>{{ t('about.faq1.q') }}</summary>
        <p>{{ t('about.faq1.a') }}</p>
      </details>
      <details>
        <summary>{{ t('about.faq2.q') }}</summary>
        <p>{{ t('about.faq2.a') }}</p>
      </details>
      <details>
        <summary>{{ t('about.faq3.q') }}</summary>
        <p>{{ t('about.faq3.a') }}</p>
      </details>
      <details>
        <summary>{{ t('about.faq4.q') }}</summary>
        <p>{{ t('about.faq4.a') }}</p>
      </details>
    </section>
    <form method="dialog" class="about-actions" @submit="close">
      <button type="submit" class="btn-primary" autofocus>{{ t('common.close') }}</button>
    </form>
  </dialog>
</template>
