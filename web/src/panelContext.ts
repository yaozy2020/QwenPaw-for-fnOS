import { inject, type InjectionKey } from 'vue';
import type { PanelContext } from './main';

export const panelKey: InjectionKey<PanelContext> = Symbol('management-panel');
export function usePanel() {
  const panel = inject(panelKey);
  if (!panel) throw new Error('Missing management panel context');
  return panel;
}
