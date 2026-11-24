import { computed, nextTick, ref } from 'vue';
import { createMemoryHistory, createRouter, isNavigationFailure, NavigationFailureType, type Router } from 'vue-router';
import App from './App.vue';
import { t, type MessageKey } from './i18n';

export const sectionOrder = ['overview', 'configuration', 'authorization', 'logs'] as const;
export type Section = (typeof sectionOrder)[number];
export type ConsoleView = Section | 'version';

const sectionLabelKeys: Record<Section, MessageKey> = {
  overview: 'nav.overview',
  configuration: 'nav.configuration',
  authorization: 'nav.authorization',
  logs: 'nav.logs',
};

export function sectionLabel(id: Section): string {
  return t(sectionLabelKeys[id]);
}

function isView(value: unknown): value is ConsoleView {
  return value === 'version' || typeof value === 'string' && Object.prototype.hasOwnProperty.call(sectionLabelKeys, value);
}

export function createConsoleRouter(): Router {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', redirect: { name: 'overview' } },
      { path: '/overview', name: 'overview', component: App, meta: { view: 'overview' } },
      { path: '/configuration', name: 'configuration', component: App, meta: { view: 'configuration' } },
      { path: '/authorization', name: 'authorization', component: App, meta: { view: 'authorization' } },
      { path: '/logs', name: 'logs', component: App, meta: { view: 'logs' } },
      { path: '/version', name: 'version', component: App, meta: { view: 'version' } },
    ],
  });
}

export function createNavigation(router: Router, onView: (view: ConsoleView | null) => void) {
  const current = ref<ConsoleView | null>(null);
  const section = computed(() => current.value === 'version' ? 'overview' : current.value);
  let removeAfterEach: (() => void) | null = null;
  let removeErrorHandler: (() => void) | null = null;
  let failed = false;

  function stop() {
    window.removeEventListener('hashchange', syncFromHash);
    window.removeEventListener('popstate', syncFromHash);
    removeAfterEach?.();
    removeAfterEach = null;
    removeErrorHandler?.();
    removeErrorHandler = null;
  }
  function fallback(error: unknown) {
    failed = true;
    stop();
    current.value = null;
    delete document.documentElement.dataset.mobileSection;
    console.warn('navigation unavailable', error);
    onView(null);
  }
  function sync(value: unknown) {
    if (failed) return;
    const view = isView(value) ? value : 'overview';
    const requested = location.hash.slice(1);
    if (requested !== view) {
      try { history.replaceState(history.state, '', `#${view}`); }
      catch (error) { fallback(error); return; }
    }
    if (current.value === view) return;
    const changed = current.value !== null;
    current.value = view;
    document.documentElement.dataset.mobileSection = section.value ?? 'overview';
    onView(view);
    if (changed || view === 'version') void nextTick(() => {
      if (current.value !== view) return;
      const target = document.getElementById(view);
      if (!target) return;
      if (changed) target.focus({ preventScroll: true });
      if (view === 'version') target.scrollIntoView({ block: 'start' });
      else window.scrollTo(0, 0);
    });
  }
  function routerFailed(error: unknown) {
    if (isNavigationFailure(error, NavigationFailureType.cancelled | NavigationFailureType.duplicated)) return;
    fallback(error);
  }
  function syncFromHash() {
    const requested = location.hash.slice(1);
    const view = isView(requested) ? requested : 'overview';
    if (router.currentRoute.value.meta.view === view) {
      sync(view);
      return;
    }
    void router.push({ name: view }).catch(routerFailed);
  }
  function navigate(id: Section, event?: MouseEvent) {
    if (current.value === null || (event && (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey))) return;
    if (current.value !== id) {
      try { history.pushState(null, '', `#${id}`); }
      catch (error) { fallback(error); return; }
      sync(id);
      void router.push({ name: id }).catch(routerFailed);
    }
    event?.preventDefault();
  }
  function start() {
    stop();
    failed = false;
    try { history.replaceState(history.state, '', location.href); }
    catch (error) { fallback(error); return; }
    window.addEventListener('hashchange', syncFromHash);
    window.addEventListener('popstate', syncFromHash);
    removeAfterEach = router.afterEach(to => {
      const requested = location.hash.slice(1);
      const view = isView(requested) ? requested : 'overview';
      if (to.meta.view === view) {
        sync(view);
      } else if (router.currentRoute.value.meta.view === view) {
        sync(view);
      } else {
        void router.push({ name: view }).catch(routerFailed);
      }
    });
    removeErrorHandler = router.onError(fallback);
    syncFromHash();
  }
  return {
    start, stop, navigate,
    get section() { return section.value; },
    get title() { return sectionLabel(section.value ?? 'overview'); },
    hides(id: Section) { return section.value !== null && section.value !== id; },
  };
}
