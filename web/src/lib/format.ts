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

/**
 * Absolute local date+time for a timestamp, used as the `title=` tooltip that
 * pairs with a relative age (see the `Time` component). "" for empty/invalid.
 */
export function formatAbsolute(iso: string | null | undefined): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
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

/** Human-readable byte size (base-1024): "—", "512 B", "1.4 KB", "23.0 MB". */
export function humanSize(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

/** Relative age from an ISO timestamp: "just now", "5m ago", "3h ago", "2d ago", else a date. */
export function relativeAge(iso: string | null | undefined): string {
  if (!iso) return '—';
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return '—';
  const secs = Math.max(0, Math.floor((Date.now() - t) / 1000));
  if (secs < 45) return 'just now';
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  return formatDate(iso);
}

/** RTP fraction (0.965) → "96.50%". Tolerant of null/invalid. */
export function formatRtp(rtp: number | null | undefined): string {
  if (rtp == null || !Number.isFinite(rtp)) return '—';
  return `${(rtp * 100).toFixed(2)}%`;
}

/**
 * Signed RTP delta in percentage points between two fractions: "+0.50", "−1.20",
 * "0.00". Uses a real minus sign for negatives. Returns "—" for invalid input.
 */
export function formatRtpDelta(before: number | null | undefined, after: number | null | undefined): string {
  if (before == null || after == null || !Number.isFinite(before) || !Number.isFinite(after)) return '—';
  const pp = (after - before) * 100;
  if (pp === 0) return '0.00';
  const s = Math.abs(pp).toFixed(2);
  return pp > 0 ? `+${s}` : `−${s}`;
}

/** Max-win multiplier → "×5,000". Tolerant of null/invalid. */
export function formatMultiplier(x: number | null | undefined): string {
  if (x == null || !Number.isFinite(x)) return '—';
  return `×${x.toLocaleString(undefined, { maximumFractionDigits: 2 })}`;
}

/** Bet cost per mode → grouped number ("1", "100", "1.5"). Tolerant of null/invalid. */
export function formatCost(cost: number | null | undefined): string {
  if (cost == null || !Number.isFinite(cost)) return '—';
  return cost.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

// --- Compliance / math-report helpers (M8) ---------------------------------

/** Fraction → percent with `dp` decimals (default 2): 0.0123 → "1.23%". Null-safe. */
export function pct(frac: number | null | undefined, dp = 2): string {
  if (frac == null || !Number.isFinite(frac)) return '—';
  return `${(frac * 100).toFixed(dp)}%`;
}

/**
 * Odds denominator N → "1 in N". Values ≥ 100,000 collapse to millions with 2dp
 * ("1 in 0.68M", "1 in 6.80M"); smaller values are rounded and grouped
 * ("1 in 1,470"). Null / non-positive input is an em-dash.
 */
export function formatOdds(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n <= 0) return '—';
  if (n >= 100_000) {
    return `1 in ${(n / 1e6).toLocaleString(undefined, {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2
    })}M`;
  }
  return `1 in ${Math.round(n).toLocaleString()}`;
}

/** Spin count (avg between events, or a worst-case streak) → grouped, up to 1dp. */
export function formatSpins(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—';
  return n.toLocaleString(undefined, { maximumFractionDigits: 1 });
}

/** Grouped number, up to `dp` decimals with trailing zeros trimmed: 6750 → "6,750". */
export function formatMetric(n: number | null | undefined, dp = 2): string {
  if (n == null || !Number.isFinite(n)) return '—';
  return n.toLocaleString(undefined, { maximumFractionDigits: dp });
}

/** Bet multiplier with an "x" suffix ("6,750x", "0.96x"); em-dash when absent. */
export function xmult(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—';
  return `${formatMetric(n)}x`;
}

/** Grouped integer for counts (entries, unique payouts, bucket counts). Null-safe. */
export function formatCount(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—';
  return Math.round(n).toLocaleString();
}

/** Sanitize a `?next=` redirect target: only allow internal absolute paths. */
export function safeNext(next: string | null | undefined, fallback = '/'): string {
  if (!next) return fallback;
  // Reject protocol-relative (//host) and absolute URLs to avoid open redirects.
  if (!next.startsWith('/') || next.startsWith('//')) return fallback;
  return next;
}
