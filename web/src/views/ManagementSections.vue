<script setup lang="ts">
import ConfigurationFields from "./ConfigurationFields.vue";
import AuthorizationFields from "./AuthorizationFields.vue";
import { usePanel } from "../panelContext";
import { t } from "../i18n";
const panel = usePanel();
</script>

<template>
  <section class="card" id="configuration" tabindex="-1" data-mobile-panel="configuration" :hidden="panel.navigation.hides('configuration')" aria-labelledby="configurationTitle">
    <div class="card-title">
      <h2 class="card-title-left" id="configurationTitle">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <rect x="3" y="11" width="18" height="11" rx="2" ry="2"/>
          <path d="M7 11V7a5 5 0 0110 0v4"/>
        </svg>
        {{ t('config.title') }}
      </h2>
    </div>
    <div v-if="panel.visited.configuration" id="configurationFields"><ConfigurationFields :state="panel.management" @service-port="panel.settings.changeServicePort" @proxy-port="panel.settings.changeWebProxyPort" @scope="panel.settings.setWebProxyPublic" @use-native="panel.settings.useNativeLogin" @migrate="panel.settings.migrateEntry" @reset="panel.settings.resetAuth" /></div>
  </section>

  <section class="card" id="authorization" tabindex="-1" data-mobile-panel="authorization" :hidden="panel.navigation.hides('authorization')" aria-labelledby="authorizationTitle">
    <div class="card-title">
      <h2 class="card-title-left" id="authorizationTitle">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <path d="M3 7a2 2 0 012-2h5l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z"/>
        </svg>
        {{ t('authz.title') }}
      </h2>
    </div>
    <div v-if="panel.visited.authorization" class="card-body" id="authorizationFields"><AuthorizationFields :state="panel.dirs.state" :service="panel.management" @refresh="panel.dirs.refresh" @add="panel.dirs.add" @open="panel.dirs.openDirectory" @reauthorize="panel.dirs.reauthorize" @remove="panel.dirs.remove" @backup="panel.openBackup" /></div>
  </section>
</template>
