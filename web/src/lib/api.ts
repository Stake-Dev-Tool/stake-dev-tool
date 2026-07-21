/**
 * web/src/lib/api.ts
 *
 * Hand-written typed client against the M1 API contract (identity & workspaces).
 * This is the ONLY place that talks to the network — every page/component imports
 * types and calls from here, so when field names shift at integration only this
 * file changes.
 *
 * RECONCILED against the generated bindings in `ui/src/lib/protocol` (ts-rs
 * output from `crates/protocol`) at M1 integration. Notes from that pass:
 * - a member's `id` IS the user id (`users.id`), so it matches both
 *   `session.user.id` and the `/members/:user_id` path parameter;
 * - accept is `POST /invites/:token/accept` (token in path, no body);
 * - the server encodes "unlimited" invite uses as `max_uses: 0` — normalized
 *   to `null` here.
 * The defensive `normalize*` helpers stay: they are what makes future server
 * shape changes a one-file fix.
 *
 * Auth is a same-origin HttpOnly session cookie. We never read it — every
 * request just sends `credentials: 'same-origin'`.
 */

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type Role = 'owner' | 'admin' | 'member';
/** Roles that can be assigned to an invite (owners are never invited in). */
export type InviteRole = Exclude<Role, 'owner'>;

export interface User {
  id: string;
  email: string;
  display_name: string;
}

export interface Workspace {
  id: string;
  slug: string;
  name: string;
}

export interface WorkspaceMembership {
  workspace: Workspace;
  role: Role;
}

export interface Member {
  user_id: string;
  display_name: string;
  role: Role;
  email?: string | null;
}

export interface WorkspaceDetail {
  workspace: Workspace;
  /** The calling user's own role, when the server provides it at the top level. */
  role: Role | null;
  members: Member[];
}

export interface Invite {
  id: string;
  role: Role;
  created_at: string;
  expires_at: string | null;
  uses: number;
  max_uses: number | null;
  revoked_at: string | null;
}

/** Returned once by POST /workspaces/:slug/invites — token/url shown a single time. */
export interface CreatedInvite {
  invite_url: string;
  token: string;
  role: Role;
  expires_at: string | null;
  max_uses: number | null;
}

export interface InvitePreview {
  workspace_name: string;
  role: Role;
  inviter_display_name: string;
  valid: boolean;
}

export interface AuthProviders {
  password: boolean;
  github: boolean;
}

export type TokenScope = 'full' | 'push:math';

export interface ApiToken {
  id: string;
  name: string;
  scopes: string[];
  created_at: string;
  expires_at: string | null;
  last_used_at?: string | null;
}

/** Returned once by POST /tokens — the secret is shown a single time. */
export interface CreatedToken extends ApiToken {
  token: string;
}

// ---------------------------------------------------------------------------
// Games & math revisions (M2)
// ---------------------------------------------------------------------------

/** Status of the server-computed per-revision bet-stats. */
export type StatsStatus = 'pending' | 'ok' | 'error';

export interface Game {
  id: string;
  slug: string;
  name: string;
  /** Newest revision number, or null when the game has no revisions yet. */
  head_number: number | null;
  revisions_count: number;
  created_at: string;
}

/** A row in a game's revision list (newest first). */
export interface RevisionSummary {
  number: number;
  message: string;
  author_display_name: string | null;
  created_at: string;
  files_count: number;
  total_size: number;
  stats_status: StatsStatus | null;
}

export interface RevisionFile {
  path: string;
  hash: string;
  size: number;
}

/** One bet mode's computed stats (RTP as a fraction, max_win as a multiplier). */
export interface StatsMode {
  mode: string;
  cost: number;
  rtp: number;
  max_win: number;
  entries: number | null;
}

export interface RevisionStats {
  status: StatsStatus;
  error: string | null;
  modes: StatsMode[];
}

export interface RevisionDetail {
  number: number;
  message: string;
  author_display_name: string | null;
  created_at: string;
  files: RevisionFile[];
  /** null when the server carries no stats object for this revision. */
  stats: RevisionStats | null;
}

/** One file entry in a diff. `size` is set for added/removed; `before_size`/`after_size` for changed. */
export interface DiffFile {
  path: string;
  size: number | null;
  before_size: number | null;
  after_size: number | null;
}

export interface DiffFiles {
  added: DiffFile[];
  removed: DiffFile[];
  changed: DiffFile[];
  unchanged: number;
}

export interface DiffModeSide {
  cost: number;
  rtp: number;
  max_win: number;
}

/** One mode compared across revisions. A side is null when the mode is new/removed. */
export interface DiffMode {
  mode: string;
  before: DiffModeSide | null;
  after: DiffModeSide | null;
}

export interface RevisionDiff {
  files: DiffFiles;
  /** modes is empty when neither revision has computed stats (show "unavailable"). */
  stats: { modes: DiffMode[] };
}

// ---------------------------------------------------------------------------
// Billing & subscriptions (M7)
// ---------------------------------------------------------------------------

/** A purchasable plan (mirrors the generated `PlanId` binding). */
export type PlanId = 'solo' | 'team';
/** Billing cadence chosen at checkout (mirrors the generated `BillingInterval`). */
export type BillingInterval = 'monthly' | 'yearly';

/**
 * The resolved plan label the status endpoint reports. `unlimited` = billing
 * disabled (self-host, everything unlimited); `expired` = trial lapsed with no
 * subscription (reads work, writes are blocked with `upgrade_required`). Kept
 * open to `string` so an unknown server value is never a runtime throw.
 */
export type PlanLabel = 'trial' | 'solo' | 'team' | 'unlimited' | 'expired';

/**
 * Current resource usage. The wire fields are `bigint` in the generated
 * bindings; we coerce to plain numbers (a 50 GiB cap is ~5.4e10, far under
 * `Number.MAX_SAFE_INTEGER`) so `humanSize`/arithmetic just work.
 */
export interface BillingUsage {
  members: number;
  storage_bytes: number;
  active_share_links: number;
}

/** Effective quota limits; `null` on any field means unlimited. */
export interface BillingLimits {
  max_members: number | null;
  max_storage_bytes: number | null;
  max_active_share_links: number | null;
  max_concurrent_share_sessions: number | null;
}

/** `GET /workspaces/:slug/billing` — member-visible, always reachable. */
export interface BillingStatus {
  /** Whether Polar billing is configured on this instance. `false` → unlimited. */
  enabled: boolean;
  plan: PlanLabel | string;
  /** Polar's status verbatim ("active", "trialing", "past_due", …) or null. */
  status: string | null;
  interval: BillingInterval | null;
  /** Period end (subscription) or trial expiry (trial); null when neither applies. */
  current_period_end: string | null;
  usage: BillingUsage;
  limits: BillingLimits;
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/**
 * Every non-2xx response is thrown as an ApiError. `code` is the machine string
 * from `{ error: { code, message } }` (e.g. "email_taken", "invalid_credentials",
 * "slug_taken", "invalid_slug"); falls back to a synthetic code when the body
 * isn't the expected shape.
 */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  /**
   * The parsed response body (when any). Lets callers read fields the envelope
   * carries alongside `error` — notably the top-level `missing` array a 409
   * `missing_blobs` returns from the revision-commit endpoint.
   */
  readonly details: unknown;
  constructor(status: number, code: string, message: string, details?: unknown) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

/** True when the failure is specifically an unauthenticated one (redirect to login). */
export function isUnauthorized(e: unknown): boolean {
  return e instanceof ApiError && e.status === 401;
}

/**
 * True when a write was refused for a billing reason: `upgrade_required` (the
 * workspace's trial lapsed, or its member cap is reached on invite-accept) or
 * `storage_quota_exceeded`. Callers surface an inline "Upgrade →" affordance
 * for these instead of a bare error.
 */
export function isUpgradeError(e: unknown): boolean {
  return (
    e instanceof ApiError &&
    (e.code === 'upgrade_required' || e.code === 'storage_quota_exceeded')
  );
}

// ---------------------------------------------------------------------------
// Core request helper
// ---------------------------------------------------------------------------

const BASE = '/api';

type Method = 'GET' | 'POST' | 'PATCH' | 'DELETE' | 'PUT';

/** Parse a response body as JSON, tolerating empty or non-JSON bodies. */
async function readJson(res: Response): Promise<unknown> {
  const raw = await res.text();
  if (!raw) return undefined;
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

/** Build an ApiError from a failed response and its already-parsed body. */
function errorFrom(status: number, statusText: string, data: unknown): ApiError {
  const err = (data as { error?: { code?: string; message?: string } } | undefined)?.error;
  return new ApiError(
    status,
    err?.code ?? `http_${status}`,
    err?.message ?? (statusText || 'Request failed'),
    data
  );
}

async function request<T>(method: Method, path: string, body?: unknown): Promise<T> {
  const hasBody = body !== undefined;
  let res: Response;
  try {
    res = await fetch(`${BASE}${path}`, {
      method,
      credentials: 'same-origin',
      headers: hasBody ? { 'content-type': 'application/json' } : undefined,
      body: hasBody ? JSON.stringify(body) : undefined
    });
  } catch {
    // Network error / server not running.
    throw new ApiError(0, 'network_error', 'Could not reach the server. Is it running?');
  }

  const data = await readJson(res);
  if (!res.ok) throw errorFrom(res.status, res.statusText, data);
  return data as T;
}

// ---------------------------------------------------------------------------
// Defensive normalizers (isolate every shape assumption here)
// ---------------------------------------------------------------------------

function asRole(v: unknown): Role {
  return v === 'owner' || v === 'admin' || v === 'member' ? v : 'member';
}

function normalizeWorkspace(raw: unknown): Workspace {
  const w = (raw ?? {}) as Record<string, unknown>;
  // Accept either { workspace: {...} } or a flat workspace object.
  const inner = (w.workspace ?? w) as Record<string, unknown>;
  return {
    id: String(inner.id ?? ''),
    slug: String(inner.slug ?? ''),
    name: String(inner.name ?? inner.slug ?? '')
  };
}

function normalizeMembership(raw: unknown): WorkspaceMembership {
  const m = (raw ?? {}) as Record<string, unknown>;
  return {
    workspace: normalizeWorkspace(m.workspace ?? m),
    role: asRole(m.role)
  };
}

function normalizeMember(raw: unknown): Member {
  const m = (raw ?? {}) as Record<string, unknown>;
  return {
    user_id: String(m.user_id ?? m.id ?? ''),
    display_name: String(m.display_name ?? m.name ?? ''),
    role: asRole(m.role),
    email: (m.email as string | undefined) ?? null
  };
}

function normalizeUser(raw: unknown): User {
  const r = (raw ?? {}) as Record<string, unknown>;
  const u = (r.user ?? r) as Record<string, unknown>;
  return {
    id: String(u.id ?? ''),
    email: String(u.email ?? ''),
    display_name: String(u.display_name ?? u.name ?? '')
  };
}

// ---- Games & revisions -----------------------------------------------------

/** Coerce to a finite number, else a fallback (default 0). */
function num(v: unknown, fallback = 0): number {
  const n = Number(v);
  return Number.isFinite(n) ? n : fallback;
}

/** Coerce to a finite number, or null when absent/invalid. */
function numOrNull(v: unknown): number | null {
  if (v == null) return null;
  const n = Number(v);
  return Number.isFinite(n) ? n : null;
}

/** Coerce to a string, or null when absent (preserves "null author" vs ""). */
function strOrNull(v: unknown): string | null {
  return v == null ? null : String(v);
}

function asStatsStatus(v: unknown): StatsStatus | null {
  return v === 'pending' || v === 'ok' || v === 'error' ? v : null;
}

function normalizeGame(raw: unknown): Game {
  const g = (raw ?? {}) as Record<string, unknown>;
  return {
    id: String(g.id ?? ''),
    slug: String(g.slug ?? ''),
    name: String(g.name ?? g.slug ?? ''),
    head_number: numOrNull(g.head_number),
    revisions_count: num(g.revisions_count),
    created_at: String(g.created_at ?? '')
  };
}

function normalizeRevisionSummary(raw: unknown): RevisionSummary {
  const r = (raw ?? {}) as Record<string, unknown>;
  return {
    number: num(r.number),
    message: String(r.message ?? ''),
    author_display_name: strOrNull(r.author_display_name),
    created_at: String(r.created_at ?? ''),
    files_count: num(r.files_count),
    total_size: num(r.total_size),
    stats_status: asStatsStatus(r.stats_status)
  };
}

function normalizeRevisionFile(raw: unknown): RevisionFile {
  const f = (raw ?? {}) as Record<string, unknown>;
  return {
    path: String(f.path ?? ''),
    hash: String(f.hash ?? ''),
    size: num(f.size)
  };
}

function normalizeStatsMode(raw: unknown): StatsMode {
  const m = (raw ?? {}) as Record<string, unknown>;
  return {
    mode: String(m.mode ?? ''),
    cost: num(m.cost),
    rtp: num(m.rtp),
    max_win: num(m.max_win),
    entries: numOrNull(m.entries)
  };
}

function normalizeRevisionStats(raw: unknown): RevisionStats | null {
  if (raw == null) return null;
  const s = raw as Record<string, unknown>;
  const modes = Array.isArray(s.modes) ? s.modes.map(normalizeStatsMode) : [];
  return {
    status: asStatsStatus(s.status) ?? 'pending',
    error: strOrNull(s.error),
    modes
  };
}

function normalizeRevisionDetail(raw: unknown): RevisionDetail {
  const r = (raw ?? {}) as Record<string, unknown>;
  const files = Array.isArray(r.files) ? r.files.map(normalizeRevisionFile) : [];
  return {
    number: num(r.number),
    message: String(r.message ?? ''),
    author_display_name: strOrNull(r.author_display_name),
    created_at: String(r.created_at ?? ''),
    files,
    stats: normalizeRevisionStats(r.stats)
  };
}

function normalizeDiffFile(raw: unknown): DiffFile {
  const f = (raw ?? {}) as Record<string, unknown>;
  return {
    path: String(f.path ?? ''),
    size: numOrNull(f.size),
    before_size: numOrNull(f.before_size),
    after_size: numOrNull(f.after_size)
  };
}

function normalizeDiffModeSide(raw: unknown): DiffModeSide | null {
  if (raw == null) return null;
  const s = raw as Record<string, unknown>;
  return { cost: num(s.cost), rtp: num(s.rtp), max_win: num(s.max_win) };
}

function normalizeDiffMode(raw: unknown): DiffMode {
  const m = (raw ?? {}) as Record<string, unknown>;
  return {
    mode: String(m.mode ?? ''),
    before: normalizeDiffModeSide(m.before),
    after: normalizeDiffModeSide(m.after)
  };
}

function normalizeRevisionDiff(raw: unknown): RevisionDiff {
  const r = (raw ?? {}) as Record<string, unknown>;
  const filesRaw = (r.files ?? {}) as Record<string, unknown>;
  const files: DiffFiles = {
    added: (Array.isArray(filesRaw.added) ? filesRaw.added : []).map(normalizeDiffFile),
    removed: (Array.isArray(filesRaw.removed) ? filesRaw.removed : []).map(normalizeDiffFile),
    changed: (Array.isArray(filesRaw.changed) ? filesRaw.changed : []).map(normalizeDiffFile),
    unchanged: num(filesRaw.unchanged)
  };
  const statsRaw = (r.stats ?? {}) as Record<string, unknown>;
  const modes = Array.isArray(statsRaw.modes) ? statsRaw.modes.map(normalizeDiffMode) : [];
  return { files, stats: { modes } };
}

// ---- Billing (M7) ----------------------------------------------------------

function asInterval(v: unknown): BillingInterval | null {
  return v === 'monthly' || v === 'yearly' ? v : null;
}

function normalizeBillingUsage(raw: unknown): BillingUsage {
  const u = (raw ?? {}) as Record<string, unknown>;
  return {
    members: num(u.members),
    storage_bytes: num(u.storage_bytes),
    active_share_links: num(u.active_share_links)
  };
}

function normalizeBillingLimits(raw: unknown): BillingLimits {
  const l = (raw ?? {}) as Record<string, unknown>;
  return {
    max_members: numOrNull(l.max_members),
    max_storage_bytes: numOrNull(l.max_storage_bytes),
    max_active_share_links: numOrNull(l.max_active_share_links),
    max_concurrent_share_sessions: numOrNull(l.max_concurrent_share_sessions)
  };
}

function normalizeBillingStatus(raw: unknown): BillingStatus {
  const b = (raw ?? {}) as Record<string, unknown>;
  return {
    enabled: Boolean(b.enabled),
    // Default to `unlimited` (the no-restrictions plan) if the server ever omits
    // it, so a shape surprise never invents a false paywall.
    plan: String(b.plan ?? 'unlimited'),
    status: strOrNull(b.status),
    interval: asInterval(b.interval),
    current_period_end: strOrNull(b.current_period_end),
    usage: normalizeBillingUsage(b.usage),
    limits: normalizeBillingLimits(b.limits)
  };
}

/**
 * Extract a list of lowercase blob hashes from a `{ missing: [...] }` payload or
 * a bare array. Accepts either plain hash strings or `{ hash }` objects, so the
 * same helper reads both the `check` response and a 409 `missing_blobs` body.
 */
function normalizeHashList(raw: unknown): string[] {
  const arr = Array.isArray(raw)
    ? raw
    : ((raw as { missing?: unknown } | null | undefined)?.missing ?? []);
  if (!Array.isArray(arr)) return [];
  const out: string[] = [];
  for (const v of arr) {
    const h = typeof v === 'string' ? v : (v as { hash?: unknown } | null)?.hash;
    if (typeof h === 'string' && h.length > 0) out.push(h.toLowerCase());
  }
  return out;
}

// ---------------------------------------------------------------------------
// Slug helpers (client-side mirror of the server rule)
// ---------------------------------------------------------------------------

/** Server rule: ^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$ (3–40 chars, no leading/trailing/`--` edges). */
export const SLUG_RE = /^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$/;

export function isValidSlug(slug: string): boolean {
  return SLUG_RE.test(slug);
}

/** Live-derive a candidate slug from a workspace name. */
export function slugFromName(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 40)
    .replace(/-+$/g, '');
}

// ---------------------------------------------------------------------------
// API surface — grouped by domain
// ---------------------------------------------------------------------------

export const api = {
  auth: {
    async register(email: string, password: string, display_name: string): Promise<User> {
      const r = await request<unknown>('POST', '/auth/register', { email, password, display_name });
      return normalizeUser(r);
    },
    async login(email: string, password: string): Promise<User> {
      const r = await request<unknown>('POST', '/auth/login', { email, password });
      return normalizeUser(r);
    },
    async logout(): Promise<void> {
      await request<void>('POST', '/auth/logout');
    },
    async me(): Promise<User> {
      const r = await request<unknown>('GET', '/auth/me');
      return normalizeUser(r);
    },
    async providers(): Promise<AuthProviders> {
      const r = await request<Partial<AuthProviders>>('GET', '/auth/providers');
      return { password: r.password ?? true, github: r.github ?? false };
    },
    /** Full-page navigation target for GitHub OAuth (never fetched). */
    githubStartUrl(): string {
      return `${BASE}/auth/github/start`;
    }
  },

  workspaces: {
    async list(): Promise<WorkspaceMembership[]> {
      const raw = await request<unknown>('GET', '/workspaces');
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { workspaces?: unknown[] } | undefined)?.workspaces ?? []);
      return arr.map(normalizeMembership);
    },
    async create(name: string, slug: string): Promise<Workspace> {
      const r = await request<unknown>('POST', '/workspaces', { name, slug });
      return normalizeWorkspace(r);
    },
    async get(slug: string): Promise<WorkspaceDetail> {
      const raw = (await request<unknown>('GET', `/workspaces/${encodeURIComponent(slug)}`)) as Record<
        string,
        unknown
      >;
      const membersRaw = Array.isArray(raw?.members) ? (raw.members as unknown[]) : [];
      return {
        workspace: normalizeWorkspace(raw?.workspace ?? raw),
        role: raw?.role === 'owner' || raw?.role === 'admin' || raw?.role === 'member' ? raw.role : null,
        members: membersRaw.map(normalizeMember)
      };
    },
    async setMemberRole(slug: string, userId: string, role: Role): Promise<void> {
      await request<void>(
        'PATCH',
        `/workspaces/${encodeURIComponent(slug)}/members/${encodeURIComponent(userId)}`,
        { role }
      );
    },
    async removeMember(slug: string, userId: string): Promise<void> {
      await request<void>(
        'DELETE',
        `/workspaces/${encodeURIComponent(slug)}/members/${encodeURIComponent(userId)}`
      );
    }
  },

  games: {
    async list(slug: string): Promise<Game[]> {
      const raw = await request<unknown>('GET', `/workspaces/${encodeURIComponent(slug)}/games`);
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { games?: unknown[] } | undefined)?.games ?? []);
      return arr.map(normalizeGame);
    },
    async revisions(slug: string, game: string): Promise<RevisionSummary[]> {
      const raw = await request<unknown>(
        'GET',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/revisions`
      );
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { revisions?: unknown[] } | undefined)?.revisions ?? []);
      return arr.map(normalizeRevisionSummary);
    },
    async revision(slug: string, game: string, number: number | string): Promise<RevisionDetail> {
      const raw = await request<unknown>(
        'GET',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/revisions/${encodeURIComponent(String(number))}`
      );
      return normalizeRevisionDetail(raw);
    },
    /** Diff of `a` (after / :number) against `b` (before / :other). */
    async diff(
      slug: string,
      game: string,
      a: number | string,
      b: number | string
    ): Promise<RevisionDiff> {
      const raw = await request<unknown>(
        'GET',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/revisions/${encodeURIComponent(String(a))}/diff/${encodeURIComponent(String(b))}`
      );
      return normalizeRevisionDiff(raw);
    },

    // ---- Browser push (M2 write path) -------------------------------------
    // Session-cookie auth already carries the implicit `full` scope, which
    // satisfies `push:math`, so the browser calls these directly (no token).

    /**
     * Validate a manifest and learn which blobs the server still needs. Returns
     * the missing hashes (lowercase hex); an empty array means every blob is
     * already stored (the whole revision deduplicated).
     */
    async check(slug: string, game: string, files: RevisionFile[]): Promise<string[]> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/revisions/check`,
        { files }
      );
      return normalizeHashList(raw);
    },

    /**
     * Stream-upload one blob's raw bytes. Uses `fetch` directly (not the JSON
     * `request` helper): a `Blob`/`File` body is streamed by the browser, so a
     * multi-GB book is never buffered in memory. Resolves 'created' (201) or
     * 'exists' (200 — already stored). `onProgress` is best-effort — `fetch`
     * cannot report sub-file upload progress, so it fires once on completion.
     */
    async putBlob(
      slug: string,
      game: string,
      hash: string,
      file: Blob,
      onProgress?: (bytesSent: number) => void
    ): Promise<'created' | 'exists'> {
      let res: Response;
      try {
        res = await fetch(
          `${BASE}/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/blobs/${encodeURIComponent(hash)}`,
          {
            method: 'PUT',
            credentials: 'same-origin',
            headers: { 'content-type': 'application/octet-stream' },
            body: file
          }
        );
      } catch {
        throw new ApiError(0, 'network_error', 'Could not reach the server. Is it running?');
      }
      const data = await readJson(res);
      if (!res.ok) {
        // Diagnostic breadcrumbs for upload failures: which status, and —
        // decisive for 413s — whether the response came from Cloudflare's
        // proxy (its free plan caps uploads at 100 MB) instead of our server.
        const cfRay = res.headers.get('cf-ray');
        console.error(
          `[push] PUT blob ${hash.slice(0, 12)}… (${file.size} B) -> ${res.status}`,
          cfRay ? `via CLOUDFLARE proxy (cf-ray ${cfRay}) — stale DNS?` : 'direct server',
          data ?? ''
        );
        const err = errorFrom(res.status, res.statusText, data);
        if (res.status === 413 && cfRay) {
          throw new ApiError(
            413,
            'cloudflare_proxy_limit',
            "Your browser is still reaching Cloudflare's proxy (100 MB upload cap). " +
              'Flush your DNS cache and fully restart the browser, then retry.'
          );
        }
        throw err;
      }
      onProgress?.(file.size);
      return res.status === 201 ? 'created' : 'exists';
    },

    /**
     * Commit a revision from an already-uploaded manifest. `parent_number` is the
     * game's current head (null for a brand-new game — the commit creates the game
     * slug implicitly). Throws ApiError on 409 `missing_blobs` (its `.details.missing`
     * lists the hashes to re-upload) or `stale_parent`, 422 `invalid_manifest` /
     * `hash_mismatch`, and 413 `payload_too_large`.
     */
    async commit(
      slug: string,
      game: string,
      input: { message: string; files: RevisionFile[]; parent_number: number | null }
    ): Promise<{ number: number }> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/revisions`,
        input
      );
      const r = (raw ?? {}) as Record<string, unknown>;
      const rev = (r.revision ?? r) as Record<string, unknown>;
      return { number: num(rev.number) };
    }
  },

  invites: {
    async list(slug: string): Promise<Invite[]> {
      const raw = await request<unknown>('GET', `/workspaces/${encodeURIComponent(slug)}/invites`);
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { invites?: unknown[] } | undefined)?.invites ?? []);
      return arr.map((v) => {
        const i = (v ?? {}) as Record<string, unknown>;
        return {
          id: String(i.id ?? ''),
          role: asRole(i.role),
          created_at: String(i.created_at ?? ''),
          expires_at: (i.expires_at as string | null) ?? null,
          uses: Number(i.uses ?? 0),
          // Server convention: 0 = unlimited.
          max_uses: !i.max_uses ? null : Number(i.max_uses),
          revoked_at: (i.revoked_at as string | null) ?? null
        } satisfies Invite;
      });
    },
    async create(
      slug: string,
      opts: { role: InviteRole; expires_in_days?: number; max_uses?: number }
    ): Promise<CreatedInvite> {
      const r = (await request<unknown>('POST', `/workspaces/${encodeURIComponent(slug)}/invites`, opts)) as Record<
        string,
        unknown
      >;
      // The secret (invite_url/token) is top-level; metadata may be flat or
      // nested under `info` (the generated `CreatedInvite { invite_url, token,
      // info }` shape). Read from whichever is present.
      const info = (r.info ?? r) as Record<string, unknown>;
      return {
        invite_url: String(r.invite_url ?? r.url ?? ''),
        token: String(r.token ?? ''),
        role: asRole(info.role),
        expires_at: (info.expires_at as string | null) ?? null,
        // Server convention: 0 = unlimited.
        max_uses: !info.max_uses ? null : Number(info.max_uses)
      };
    },
    async revoke(slug: string, id: string): Promise<void> {
      await request<void>(
        'DELETE',
        `/workspaces/${encodeURIComponent(slug)}/invites/${encodeURIComponent(id)}`
      );
    },
    /** Public, unauthenticated preview of an invite token. */
    async preview(token: string): Promise<InvitePreview> {
      const r = (await request<unknown>('GET', `/invites/${encodeURIComponent(token)}`)) as Record<
        string,
        unknown
      >;
      return {
        workspace_name: String(r.workspace_name ?? ''),
        role: asRole(r.role),
        inviter_display_name: String(r.inviter_display_name ?? ''),
        valid: Boolean(r.valid ?? false)
      };
    },
    /**
     * Accept an invite (requires a browser session). The response is
     * `{ workspace: WorkspaceSummary }` where the summary carries the caller's
     * role and the slug, so callers can navigate straight to /w/[slug].
     */
    async accept(token: string): Promise<WorkspaceMembership> {
      const r = (await request<unknown>(
        'POST',
        `/invites/${encodeURIComponent(token)}/accept`
      )) as Record<string, unknown>;
      const summary = (r.workspace ?? r) as Record<string, unknown>;
      return {
        workspace: normalizeWorkspace(summary),
        role: asRole(summary.role)
      };
    }
  },

  tokens: {
    async list(): Promise<ApiToken[]> {
      const raw = await request<unknown>('GET', '/tokens');
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { tokens?: unknown[] } | undefined)?.tokens ?? []);
      return arr.map((v) => {
        const t = (v ?? {}) as Record<string, unknown>;
        return {
          id: String(t.id ?? ''),
          name: String(t.name ?? ''),
          scopes: Array.isArray(t.scopes) ? t.scopes.map(String) : [],
          created_at: String(t.created_at ?? ''),
          expires_at: (t.expires_at as string | null) ?? null,
          last_used_at: (t.last_used_at as string | null) ?? null
        } satisfies ApiToken;
      });
    },
    async create(opts: { name: string; scopes: string[]; expires_in_days?: number }): Promise<CreatedToken> {
      const r = (await request<unknown>('POST', '/tokens', opts)) as Record<string, unknown>;
      // Secret is top-level; metadata may be flat or nested under `info` (the
      // generated `CreatedToken { token, info }` shape).
      const info = (r.info ?? r) as Record<string, unknown>;
      return {
        id: String(info.id ?? ''),
        name: String(info.name ?? opts.name),
        scopes: Array.isArray(info.scopes) ? info.scopes.map(String) : opts.scopes,
        created_at: String(info.created_at ?? ''),
        expires_at: (info.expires_at as string | null) ?? null,
        last_used_at: (info.last_used_at as string | null) ?? null,
        token: String(r.token ?? '')
      };
    },
    async remove(id: string): Promise<void> {
      await request<void>('DELETE', `/tokens/${encodeURIComponent(id)}`);
    }
  },

  billing: {
    /**
     * Member-visible plan / usage / limits. Always reachable — on a self-hosted
     * instance it returns `enabled: false` with every limit unlimited.
     */
    async status(slug: string): Promise<BillingStatus> {
      const raw = await request<unknown>('GET', `/workspaces/${encodeURIComponent(slug)}/billing`);
      return normalizeBillingStatus(raw);
    },
    /**
     * Owner-only: start a Polar checkout for `plan`/`interval`. Returns the
     * hosted checkout URL to navigate to (`window.location.href = url`). The
     * endpoint 404s when billing is disabled on the instance.
     */
    async checkout(slug: string, plan: PlanId, interval: BillingInterval): Promise<string> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/billing/checkout`,
        { plan, interval }
      );
      const r = (raw ?? {}) as Record<string, unknown>;
      return String(r.checkout_url ?? '');
    }
  },

  device: {
    /** Approve or deny a desktop/CLI device-authorization request (requires auth). */
    async approve(user_code: string, approve: boolean): Promise<void> {
      await request<void>('POST', '/auth/device/approve', { user_code, approve });
    }
  }
};
