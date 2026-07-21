/**
 * web/src/lib/clipboard.ts — one place to copy text and confirm it.
 *
 * Writes to the clipboard and raises a global toast, so every copy affordance
 * (CopyField, inline hash/id copies) gives the same feedback. Returns whether
 * the write succeeded, letting callers keep any local "Copied" button state.
 */
import { toast } from './toasts.svelte';

export async function copyText(text: string, label = 'Copied to clipboard'): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(label);
    return true;
  } catch {
    toast.error('Copy failed — copy it manually.');
    return false;
  }
}
