/**
 * web/src/lib/toasts.svelte.ts
 *
 * A tiny global toast store. A single `$state` list of active toasts, mutated
 * through `toast.success|error|info`, rendered once by `<Toasts/>` in the root
 * layout. Toasts auto-dismiss (errors linger a little longer) and can be closed
 * by hand. Kept logic-only — no DOM — so any module can raise one.
 */

export type ToastKind = 'success' | 'error' | 'info';

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

export const toastState = $state<{ items: Toast[] }>({ items: [] });

let seq = 0;
const timers = new Map<number, ReturnType<typeof setTimeout>>();

function add(kind: ToastKind, message: string, duration: number): number {
  const id = ++seq;
  toastState.items = [...toastState.items, { id, kind, message }];
  if (duration > 0) timers.set(id, setTimeout(() => dismiss(id), duration));
  return id;
}

/** Remove a toast (and clear its auto-dismiss timer) by id. */
export function dismiss(id: number): void {
  const t = timers.get(id);
  if (t) {
    clearTimeout(t);
    timers.delete(id);
  }
  toastState.items = toastState.items.filter((x) => x.id !== id);
}

/** Raise a toast. Errors default to a longer dwell than success/info. */
export const toast = {
  success: (message: string, duration = 3500) => add('success', message, duration),
  error: (message: string, duration = 6000) => add('error', message, duration),
  info: (message: string, duration = 3500) => add('info', message, duration)
};
