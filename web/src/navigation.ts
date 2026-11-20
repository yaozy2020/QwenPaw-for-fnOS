import { computed, nextTick, ref } from 'vue';

export const sectionLabels = {
  overview: '概览', configuration: '配置', authorization: '授权与备份', logs: '日志',
} as const;
type Section = keyof typeof sectionLabels;
export type ConsoleView = Section | 'version';

function isView(value: string): value is ConsoleView {
  return value === 'version' || Object.prototype.hasOwnProperty.call(sectionLabels, value);
}

export function createNavigation(onView: (view: ConsoleView | null) => void) {
  const current = ref<ConsoleView | null>(null);
  const section = computed(() => current.value === 'version' ? 'overview' : current.value);

  function stop() {
    window.removeEventListener('hashchange', sync);
    window.removeEventListener('popstate', sync);
  }
  function sync() {
    const requested = location.hash.slice(1);
    const view = isView(requested) ? requested : 'overview';
    if (requested !== view) {
      try { history.replaceState(history.state, '', `#${view}`); }
      catch (error) {
        stop();
        current.value = null;
        delete document.documentElement.dataset.mobileSection;
        console.warn('navigation unavailable', error);
        onView(null);
        return;
      }
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
  function start() {
    window.addEventListener('hashchange', sync);
    window.addEventListener('popstate', sync);
    sync();
  }
  return {
    start, stop,
    get section() { return section.value; },
    get title() { return sectionLabels[section.value ?? 'overview']; },
    hides(id: Section) { return section.value !== null && section.value !== id; },
  };
}
