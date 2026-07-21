/**
 * web/src/lib/format.ts — small presentation helpers (no network, no state).
 */
import type { Role } from './api';

/** Map a workspace role to a Badge tone. */
export function roleTone(role: Role): 'accent' | 'info' | 'neutral' {
  if (role === 'owner') return 'accent';
  if (role === 'admin') return 'info';
  return 'neutral';
}

/** Format an ISO timestamp as a short local date, tolerant of empty/invalid input. */
export function formatDate(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '—';
  return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

/** Human "expires" string: never / relative-ish date / expired. */
export function formatExpiry(iso: string | null | undefined): string {
  if (!iso) return 'Never';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '—';
  if (d.getTime() < Date.now()) return 'Expired';
  return formatDate(iso);
}

/** Turn an unknown thrown value into a display string. */
export function errorText(e: unknown): string {
  if (e && typeof e === 'object' && 'message' in e) return String((e as { message: unknown }).message);
  return String(e);
}

/** Sanitize a `?next=` redirect target: only allow internal absolute paths. */
export function safeNext(next: string | null | undefined, fallback = '/'): string {
  if (!next) return fallback;
  // Reject protocol-relative (//host) and absolute URLs to avoid open redirects.
  if (!next.startsWith('/') || next.startsWith('//')) return fallback;
  return next;
}
