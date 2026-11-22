import { computed, nextTick, ref } from 'vue';
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

function isView(value: string): value is ConsoleView {
  return value === 'version' || Object.prototype.hasOwnProperty.call(sectionLabelKeys, value);
}

export function createNavigation(onView: (view: ConsoleView | null) => void) {
  const current = ref<ConsoleView | null>(null);
  const section = computed(() => current.value === 'version' ? 'overview' : current.value);

  function stop() {
    window.removeEventListener('hashchange', sync);
    window.removeEventListener('popstate', sync);
  }
  function fallback(error: unknown) {
    stop();
    current.value = null;
    delete document.documentElement.dataset.mobileSection;
    console.warn('navigation unavailable', error);
    onView(null);
  }
  function sync() {
    const requested = location.hash.slice(1);
    const view = isView(requested) ? requested : 'overview';
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
  function navigate(id: Section, event?: MouseEvent) {
    if (event) {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
    }
    if (current.value === null) return;
    if (current.value === id) return;
    try { history.pushState(null, '', `#${id}`); }
    catch (error) { fallback(error); return; }
    sync();
  }
  function start() {
    window.addEventListener('hashchange', sync);
    window.addEventListener('popstate', sync);
    sync();
  }
  return {
    start, stop, navigate,
    get section() { return section.value; },
    get title() { return sectionLabel(section.value ?? 'overview'); },
    hides(id: Section) { return section.value !== null && section.value !== id; },
  };
}
