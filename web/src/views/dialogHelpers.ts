import { watch, type Ref } from 'vue';

export function showDialog(el?: HTMLDialogElement | null) {
  if (!el || el.open) return;
  if (typeof el.showModal === 'function') el.showModal();
  else el.setAttribute('open', '');
}

export function closeDialog(el?: HTMLDialogElement | null) {
  if (!el) return;
  if (typeof el.close === 'function') el.close();
  else el.removeAttribute('open');
}

export function useOutsideDismiss(
  root: Ref<HTMLElement | undefined>,
  isOpen: () => boolean,
  onClose: () => void,
) {
  function onPointerDown(event: PointerEvent) {
    if (event.target instanceof Node && !root.value?.contains(event.target)) onClose();
  }
  function onFocusOut(event: FocusEvent) {
    if (event.relatedTarget instanceof Node && !root.value?.contains(event.relatedTarget)) onClose();
  }
  watch(isOpen, (shown, _, cleanup) => {
    if (!shown) return;
    document.addEventListener('pointerdown', onPointerDown, true);
    cleanup(() => document.removeEventListener('pointerdown', onPointerDown, true));
  });
  return { onFocusOut };
}
