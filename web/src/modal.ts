export type ModalInput = {
  label?: string;
  value?: string;
  type?: string;
  inputMode?: HTMLInputElement["inputMode"];
  placeholder?: string;
  pattern?: string;
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
