<script lang="ts">
  /**
   * Toasts — the single global notification stack, mounted once in the root
   * layout. Reads `toastState` and renders a bottom (mobile) / bottom-right
   * (desktop) column with an aria-live region so screen readers announce them.
   */
  import { toastState, dismiss, type ToastKind } from '$lib/toasts.svelte';

  const box: Record<ToastKind, string> = {
    success: 'border-accent/40',
    error: 'border-danger/40',
    info: 'border-info/40'
  };
  const glyph: Record<ToastKind, string> = { success: '✓', error: '!', info: 'i' };
  const glyphTone: Record<ToastKind, string> = {
    success: 'bg-accent/15 text-accent',
    error: 'bg-danger/15 text-danger',
    info: 'bg-info/15 text-info'
  };
</script>

<div
  class="pointer-events-none fixed inset-x-0 bottom-0 z-[60] flex flex-col items-center gap-2 p-4 sm:items-end"
  role="region"
  aria-live="polite"
  aria-label="Notifications"
>
  {#each toastState.items as t (t.id)}
    <div
      class="fade-in pointer-events-auto flex w-full max-w-sm items-start gap-3 rounded-lg border bg-surface px-4 py-3 text-sm text-text shadow-xl shadow-black/30 {box[
        t.kind
      ]}"
    >
      <span
        class="mt-0.5 flex h-4 w-4 flex-shrink-0 items-center justify-center rounded-full text-[10px] font-bold {glyphTone[
          t.kind
        ]}"
        aria-hidden="true">{glyph[t.kind]}</span
      >
      <span class="min-w-0 flex-1 break-words">{t.message}</span>
      <button
        type="button"
        class="flex-shrink-0 text-faint transition hover:text-text"
        aria-label="Dismiss notification"
        onclick={() => dismiss(t.id)}>✕</button
      >
    </div>
  {/each}
</div>
