import { shallowReactive } from "vue";

export type ModalInput = {
  label?: string;
  value?: string;
  type?: string;
  inputMode?: 'numeric' | 'text';
  placeholder?: string;
  pattern?: string;
  invalidMessage?: string;
  validate?: (value: string) => boolean;
};

export type ModalOptions = {
  title?: string;
  message?: string;
  type?: "primary" | "warn" | "danger";
  confirmText?: string;
  cancelText?: string;
  input?: ModalInput | null;
};

export type ModalOptionsWithInput = ModalOptions & { input: ModalInput };
export type ModalOptionsWithoutInput = ModalOptions & { input?: null };

export interface ShowModal {
  (options: ModalOptionsWithInput): Promise<string | null | false>;
  (options?: ModalOptionsWithoutInput): Promise<boolean>;
}

export function createPrompt() {
  const state = shallowReactive({ options: null as ModalOptions | null });
  let settle: ((value: boolean | string | null) => void) | null = null;
  function show(options: ModalOptionsWithInput): Promise<string | null | false>;
  function show(options?: ModalOptionsWithoutInput): Promise<boolean>;
  function show(options: ModalOptions = {}): Promise<boolean | string | null> {
    if (settle) return Promise.resolve(false);
    return new Promise(resolve => { settle = resolve; state.options = options; });
  }
  function finish(value: boolean | string | null) {
    const resolve = settle;
    settle = state.options = null;
    resolve?.(value);
  }
  return { state, show, finish };
}
