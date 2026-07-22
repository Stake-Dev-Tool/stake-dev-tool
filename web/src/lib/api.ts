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
  /** Whether the account's email address has been confirmed. */
  email_verified: boolean;
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
  /**
   * The workspace's attached custom play domain (lowercase, e.g. `play.acme.com`),
   * or null when none is set. Share links are then served at `<slug>.<domain>`.
   */
  custom_play_domain: string | null;
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
  discord: boolean;
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
  /**
   * Stake-Engine-style compliance analysis (M8). `null` on older revisions that
   * predate the analyzer — the Math report shows a "push a new revision" hint.
   */
  analysis: RevisionAnalysis | null;
}

// ---- Compliance analysis (M8: Math report) ---------------------------------
// Every numeric field is modelled as `number | null` and normalized through
// `numOrNull`: a partial payload from the analyzer never throws and any missing
// figure renders as an em-dash rather than a misleading zero.

export type Volatility = 'low' | 'medium' | 'high';

/**
 * One bet-level compliance constraint, evaluated against both the 2★ and 3★
 * reference bets. Single-value metrics carry `value` (compared to limit2/limit3
 * for each column); per-reference-bet metrics (max_exposure, max_bet_cost) carry
 * `value2`/`value3`. Range metrics (volatility) carry a `limitX_low` low bound.
 */
export interface ConstraintRow {
  key: string;
  label: string;
  value: number | null;
  value2: number | null;
  value3: number | null;
  limit2_low: number | null;
  limit2: number | null;
  limit3_low: number | null;
  limit3: number | null;
  pass2: boolean;
  pass3: boolean;
}

/** One bucket of a mode's hit-rate distribution. `to` null = open-ended (∞). */
export interface DistBucket {
  from: number | null;
  to: number | null;
  count: number | null;
  probability: number | null;
  effective_hit_rate: number | null;
  rtp_contribution: number | null;
}

/** One pass/fail compliance check for a mode (label · expected → result). */
export interface ComplianceCheck {
  check: string;
  label: string;
  expected: string;
  result: string;
  pass: boolean;
}

/**
 * Full per-mode analysis. rtp / etl / *_prob fields are fractions (render ×100);
 * max_win is a bet multiplier; streaks are spin counts; max_win_odds is the
 * "1 in N" denominator.
 */
export interface ModeAnalysis {
  mode: string;
  cost: number | null;
  rtp: number | null;
  std_dev: number | null;
  volatility: Volatility | null;
  max_win: number | null;
  min_win: number | null;
  zero_prob: number | null;
  sub_bet_prob: number | null;
  win_prob: number | null;
  break_even_miss_prob: number | null;
  hit_rate: number | null;
  unique_payouts: number | null;
  entries: number | null;
  max_win_odds: number | null;
  avg_spins_any_win: number | null;
  worst_zero_streak: number | null;
  avg_spins_profit: number | null;
  worst_loss_streak: number | null;
  tail_prob_5000: number | null;
  tail_prob_10000: number | null;
  cvar: number | null;
  etl_40: number | null;
  etl_10000: number | null;
  etl_sum: number | null;
  distribution: DistBucket[];
  compliance: ComplianceCheck[];
}

/** Per-revision compliance verdict, constraints table and per-mode analyses. */
export interface RevisionAnalysis {
  two_star_compliant: boolean;
  three_star_compliant: boolean;
  stars: 0 | 2 | 3;
  cross_mode_rtp_variance: number | null;
  cross_mode_rtp_pass: boolean;
  reference_max_bet_2: number | null;
  reference_max_bet_3: number | null;
  constraints: ConstraintRow[];
  modes: ModeAnalysis[];
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

/** Billing cadence chosen at checkout (mirrors the generated `BillingInterval`). */
export type BillingInterval = 'monthly' | 'yearly';

/**
 * The resolved plan label the status endpoint reports. `unlimited` = billing
 * disabled (self-host, everything unlimited); `free` = billing enabled with no
 * active subscription (reads work, writes are blocked with `upgrade_required`);
 * `paid` = an active seat subscription (or comp). Kept open to `string` so an
 * unknown server value is never a runtime throw.
 */
export type PlanLabel = 'free' | 'paid' | 'unlimited';

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
  /** Whether Stripe billing is configured on this instance. `false` → unlimited. */
  enabled: boolean;
  plan: PlanLabel | string;
  /** Seat count backing a `paid` plan (subscription quantity or comp); null otherwise. */
  seats: number | null;
  /** Stripe's status verbatim ("active", "trialing", "past_due", …) or null. */
  status: string | null;
  interval: BillingInterval | null;
  /** The subscription's current period end; null when there is no subscription. */
  current_period_end: string | null;
  /**
   * Extra storage granted by the add-on, in GiB (already folded into
   * `limits.max_storage_bytes`). `0` when no storage add-on is active.
   */
  extra_storage_gib: number;
  usage: BillingUsage;
  limits: BillingLimits;
}

// ---------------------------------------------------------------------------
// Share links & front bundles (M5)
// ---------------------------------------------------------------------------

/**
 * A hosted share link (`<slug>.play.<domain>`), as returned by create/list. The
 * wire counters (`sessions_count`, `spins_count`, `active_sessions`) are `bigint`
 * in the generated bindings; coerced to plain numbers here (a spin count is far
 * under `Number.MAX_SAFE_INTEGER`) so arithmetic/formatting just works.
 */
export interface ShareLink {
  id: string;
  slug: string;
  /** Full `https://<slug>.<play_domain>/`, or null when the instance has no play domain. */
  url: string | null;
  /** The game slug this link serves. */
  game: string;
  /** null = tracks the game's latest revision; else the pinned revision number. */
  revision_number: number | null;
  /** null = serves the game's latest bundle; else the pinned bundle id. */
  front_bundle_id: string | null;
  password_protected: boolean;
  expires_at: string | null;
  max_concurrent_sessions: number;
  revoked_at: string | null;
  created_at: string;
  sessions_count: number;
  spins_count: number;
  total_bet: number;
  total_win: number;
  /** `total_win / total_bet` when `total_bet > 0`, else null. */
  observed_rtp: number | null;
  /** Best-effort visitor sessions seen in the last 30 min on this node. */
  active_sessions: number;
}

/** Body for `POST …/shares`. Omitted fields take their server default. */
export interface CreateShareInput {
  /** Custom subdomain label; omit for a generated `word-word-nnn`. */
  slug?: string;
  /** Pin a revision number; omit to track the latest revision. */
  revision_number?: number;
  /** Pin a front bundle id; omit to serve the latest bundle. */
  front_bundle_id?: string;
  /** Optional password (plaintext; hashed server-side). */
  password?: string;
  /** Expiry in days from now; omit for no expiry. */
  expires_in_days?: number;
  /** Concurrent visitor-session cap; omit for the default of 25. */
  max_concurrent_sessions?: number;
}

/**
 * Body for `PATCH …/shares/:id`. Tri-state on the nullable fields: leave a key
 * ABSENT (undefined) to keep it unchanged, pass `null` to clear it (track latest
 * / remove password / never expire), or a value to set it. `undefined` keys are
 * dropped by `JSON.stringify`, so this object maps straight onto the server's
 * absent-vs-null semantics.
 */
export interface UpdateShareInput {
  revision_number?: number | null;
  front_bundle_id?: string | null;
  password?: string | null;
  expires_in_days?: number | null;
  max_concurrent_sessions?: number;
  revoked?: boolean;
}

/** `201` payload after a front bundle commits. */
export interface FrontBundleCreated {
  id: string;
  created_at: string;
}

/**
 * One front bundle in a game's bundle list (newest first). `files_count`/
 * `total_size` are `bigint` on the wire; coerced to plain numbers here (a bundle
 * is far under `Number.MAX_SAFE_INTEGER`). `is_latest` flags the newest bundle —
 * the one a latest-tracking share serves.
 */
export interface FrontBundleSummary {
  id: string;
  created_at: string;
  files_count: number;
  total_size: number;
  is_latest: boolean;
}

/**
 * Result of a content-lifecycle deletion (a revision or a front bundle): the
 * storage the blob GC reclaimed. Both fields are `bigint` on the wire; coerced to
 * plain numbers. Both are 0 when every referenced blob is still shared elsewhere.
 */
export interface DeletionResult {
  freed_bytes: number;
  freed_blobs: number;
}

// ---------------------------------------------------------------------------
// Admin console (instance operator — M-admin)
// ---------------------------------------------------------------------------
// Every /api/admin endpoint is cookie-auth AND admin-gated: a non-admin gets a
// flat 404 on ALL of them, including /me. `admin.me()` translates that 404 to
// `false` (never a throw), so gating the UI is a boolean, not an error path.

/** One day's count in a 30-day activity series (signups / pushes). */
export interface AdminDayCount {
  /** `YYYY-MM-DD` (server-local day). */
  date: string;
  count: number;
}

/**
 * Instance-wide totals plus two 30-day daily series. The count fields may be
 * `bigint` on the wire; coerced to plain numbers (well under
 * `Number.MAX_SAFE_INTEGER`) so formatting/arithmetic just work.
 */
/** Host machine capacity (disk backing storage + memory); null when unprobed. */
export interface AdminHostStats {
  disk_total_bytes: number;
  disk_free_bytes: number;
  mem_total_bytes: number;
  mem_used_bytes: number;
}

export interface AdminOverview {
  users: number;
  workspaces: number;
  games: number;
  revisions: number;
  share_links: number;
  storage_bytes: number;
  sessions_total: number;
  spins_total: number;
  host: AdminHostStats | null;
  signups_30d: AdminDayCount[];
  pushes_30d: AdminDayCount[];
}

/**
 * A comp/override an operator has granted on a workspace's plan. `plan` is the
 * comped plan label; `expires_at` null = no expiry; `note` an optional memo.
 */
export interface AdminOverrideInfo {
  plan: string;
  /** Comped seat count when `plan === 'paid'`; null for `unlimited`. */
  seats: number | null;
  expires_at: string | null;
  note: string | null;
}

/** One row in the admin workspaces table. */
export interface AdminWorkspace {
  id: string;
  slug: string;
  name: string;
  created_at: string;
  members: number;
  games: number;
  storage_bytes: number;
  /** Effective resolved plan label ("free"/"paid"/"unlimited"). */
  plan: string;
  /** Resolved seat count when the plan is `paid` (comp or subscription); null otherwise. */
  seats: number | null;
  /** Present when an operator comp is active; null otherwise. */
  override: AdminOverrideInfo | null;
  /** Stripe's verbatim subscription status, or null when there's no subscription. */
  subscription_status: string | null;
}

/** The plan an override sets; `null` clears the override entirely. */
export type AdminOverridePlan = 'paid' | 'unlimited' | null;

/**
 * Body for `PUT /admin/workspaces/:id/override`. `plan: null` clears the comp;
 * `seats` is required when `plan === 'paid'`; `expires_in_days` (relative) and
 * `note` are optional and only meaningful when granting a plan.
 */
export interface AdminOverrideInput {
  plan: AdminOverridePlan;
  seats?: number;
  expires_in_days?: number;
  note?: string;
}

/** One row in the admin users table. */
export interface AdminUser {
  id: string;
  email: string;
  display_name: string;
  created_at: string;
  is_admin: boolean;
  /** Count of workspaces the user belongs to. */
  workspaces: number;
}

/** One row in the admin shares table (cross-workspace share-link moderation). */
export interface AdminShare {
  id: string;
  slug: string;
  /** Full play URL, or null when the instance has no play domain. */
  url: string | null;
  workspace_slug: string;
  game: string;
  sessions_count: number;
  spins_count: number;
  revoked_at: string | null;
  created_at: string;
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
 * workspace has no active plan, or its member cap is reached on invite-accept) or
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
    display_name: String(u.display_name ?? u.name ?? ''),
    // Default to verified when the field is absent so an older server (or a
    // shape surprise) never shows a false "verify your email" nag.
    email_verified: Boolean(u.email_verified ?? true)
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
    modes,
    analysis: normalizeRevisionAnalysis(s.analysis)
  };
}

// ---- Compliance analysis (M8) ----------------------------------------------

function asVolatility(v: unknown): Volatility | null {
  return v === 'low' || v === 'medium' || v === 'high' ? v : null;
}

/** Coerce to the star tier the analyzer emits; anything else is 0 (non-compliant). */
function asStars(v: unknown): 0 | 2 | 3 {
  const n = num(v);
  return n === 2 ? 2 : n === 3 ? 3 : 0;
}

function normalizeConstraintRow(raw: unknown): ConstraintRow {
  const c = (raw ?? {}) as Record<string, unknown>;
  return {
    key: String(c.key ?? ''),
    label: String(c.label ?? c.key ?? ''),
    value: numOrNull(c.value),
    value2: numOrNull(c.value2),
    value3: numOrNull(c.value3),
    limit2_low: numOrNull(c.limit2_low),
    limit2: numOrNull(c.limit2),
    limit3_low: numOrNull(c.limit3_low),
    limit3: numOrNull(c.limit3),
    pass2: Boolean(c.pass2),
    pass3: Boolean(c.pass3)
  };
}

function normalizeDistBucket(raw: unknown): DistBucket {
  const b = (raw ?? {}) as Record<string, unknown>;
  return {
    from: numOrNull(b.from),
    to: numOrNull(b.to),
    count: numOrNull(b.count),
    probability: numOrNull(b.probability),
    effective_hit_rate: numOrNull(b.effective_hit_rate),
    rtp_contribution: numOrNull(b.rtp_contribution)
  };
}

function normalizeComplianceCheck(raw: unknown): ComplianceCheck {
  const c = (raw ?? {}) as Record<string, unknown>;
  return {
    check: String(c.check ?? ''),
    label: String(c.label ?? c.check ?? ''),
    expected: String(c.expected ?? ''),
    result: String(c.result ?? ''),
    pass: Boolean(c.pass)
  };
}

function normalizeModeAnalysis(raw: unknown): ModeAnalysis {
  const m = (raw ?? {}) as Record<string, unknown>;
  return {
    mode: String(m.mode ?? ''),
    cost: numOrNull(m.cost),
    rtp: numOrNull(m.rtp),
    std_dev: numOrNull(m.std_dev),
    volatility: asVolatility(m.volatility),
    max_win: numOrNull(m.max_win),
    min_win: numOrNull(m.min_win),
    zero_prob: numOrNull(m.zero_prob),
    sub_bet_prob: numOrNull(m.sub_bet_prob),
    win_prob: numOrNull(m.win_prob),
    break_even_miss_prob: numOrNull(m.break_even_miss_prob),
    hit_rate: numOrNull(m.hit_rate),
    unique_payouts: numOrNull(m.unique_payouts),
    entries: numOrNull(m.entries),
    max_win_odds: numOrNull(m.max_win_odds),
    avg_spins_any_win: numOrNull(m.avg_spins_any_win),
    worst_zero_streak: numOrNull(m.worst_zero_streak),
    avg_spins_profit: numOrNull(m.avg_spins_profit),
    worst_loss_streak: numOrNull(m.worst_loss_streak),
    tail_prob_5000: numOrNull(m.tail_prob_5000),
    tail_prob_10000: numOrNull(m.tail_prob_10000),
    cvar: numOrNull(m.cvar),
    etl_40: numOrNull(m.etl_40),
    etl_10000: numOrNull(m.etl_10000),
    etl_sum: numOrNull(m.etl_sum),
    distribution: Array.isArray(m.distribution) ? m.distribution.map(normalizeDistBucket) : [],
    compliance: Array.isArray(m.compliance) ? m.compliance.map(normalizeComplianceCheck) : []
  };
}

function normalizeRevisionAnalysis(raw: unknown): RevisionAnalysis | null {
  if (raw == null) return null;
  const a = raw as Record<string, unknown>;
  return {
    two_star_compliant: Boolean(a.two_star_compliant),
    three_star_compliant: Boolean(a.three_star_compliant),
    stars: asStars(a.stars),
    cross_mode_rtp_variance: numOrNull(a.cross_mode_rtp_variance),
    cross_mode_rtp_pass: Boolean(a.cross_mode_rtp_pass),
    reference_max_bet_2: numOrNull(a.reference_max_bet_2),
    reference_max_bet_3: numOrNull(a.reference_max_bet_3),
    constraints: Array.isArray(a.constraints) ? a.constraints.map(normalizeConstraintRow) : [],
    modes: Array.isArray(a.modes) ? a.modes.map(normalizeModeAnalysis) : []
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
    seats: numOrNull(b.seats),
    status: strOrNull(b.status),
    interval: asInterval(b.interval),
    current_period_end: strOrNull(b.current_period_end),
    extra_storage_gib: num(b.extra_storage_gib),
    usage: normalizeBillingUsage(b.usage),
    limits: normalizeBillingLimits(b.limits)
  };
}

// ---- Share links (M5) ------------------------------------------------------

function normalizeShareLink(raw: unknown): ShareLink {
  const s = (raw ?? {}) as Record<string, unknown>;
  return {
    id: String(s.id ?? ''),
    slug: String(s.slug ?? ''),
    url: strOrNull(s.url),
    game: String(s.game ?? ''),
    revision_number: numOrNull(s.revision_number),
    front_bundle_id: strOrNull(s.front_bundle_id),
    password_protected: Boolean(s.password_protected),
    expires_at: strOrNull(s.expires_at),
    max_concurrent_sessions: num(s.max_concurrent_sessions, 25),
    revoked_at: strOrNull(s.revoked_at),
    created_at: String(s.created_at ?? ''),
    sessions_count: num(s.sessions_count),
    spins_count: num(s.spins_count),
    total_bet: num(s.total_bet),
    total_win: num(s.total_win),
    observed_rtp: numOrNull(s.observed_rtp),
    active_sessions: num(s.active_sessions)
  };
}

function normalizeFrontBundleSummary(raw: unknown): FrontBundleSummary {
  const b = (raw ?? {}) as Record<string, unknown>;
  return {
    id: String(b.id ?? ''),
    created_at: String(b.created_at ?? ''),
    files_count: num(b.files_count),
    total_size: num(b.total_size),
    is_latest: Boolean(b.is_latest)
  };
}

function normalizeDeletionResult(raw: unknown): DeletionResult {
  const r = (raw ?? {}) as Record<string, unknown>;
  return { freed_bytes: num(r.freed_bytes), freed_blobs: num(r.freed_blobs) };
}

// ---- Admin console ---------------------------------------------------------

function normalizeAdminDayCount(raw: unknown): AdminDayCount {
  const d = (raw ?? {}) as Record<string, unknown>;
  return { date: String(d.date ?? ''), count: num(d.count) };
}

function normalizeAdminOverview(raw: unknown): AdminOverview {
  const o = (raw ?? {}) as Record<string, unknown>;
  return {
    users: num(o.users),
    workspaces: num(o.workspaces),
    games: num(o.games),
    revisions: num(o.revisions),
    share_links: num(o.share_links),
    storage_bytes: num(o.storage_bytes),
    sessions_total: num(o.sessions_total),
    spins_total: num(o.spins_total),
    host: normalizeAdminHost(o.host),
    signups_30d: Array.isArray(o.signups_30d) ? o.signups_30d.map(normalizeAdminDayCount) : [],
    pushes_30d: Array.isArray(o.pushes_30d) ? o.pushes_30d.map(normalizeAdminDayCount) : []
  };
}

function normalizeAdminHost(raw: unknown): AdminHostStats | null {
  if (raw == null || typeof raw !== 'object') return null;
  const h = raw as Record<string, unknown>;
  return {
    disk_total_bytes: num(h.disk_total_bytes),
    disk_free_bytes: num(h.disk_free_bytes),
    mem_total_bytes: num(h.mem_total_bytes),
    mem_used_bytes: num(h.mem_used_bytes)
  };
}

function normalizeAdminOverride(raw: unknown): AdminOverrideInfo | null {
  if (raw == null) return null;
  const o = raw as Record<string, unknown>;
  return {
    plan: String(o.plan ?? ''),
    seats: numOrNull(o.seats),
    expires_at: strOrNull(o.expires_at),
    note: strOrNull(o.note)
  };
}

function normalizeAdminWorkspace(raw: unknown): AdminWorkspace {
  const w = (raw ?? {}) as Record<string, unknown>;
  return {
    id: String(w.id ?? ''),
    slug: String(w.slug ?? ''),
    name: String(w.name ?? w.slug ?? ''),
    created_at: String(w.created_at ?? ''),
    members: num(w.members),
    games: num(w.games),
    storage_bytes: num(w.storage_bytes),
    // Default to `free` (the most-restricted state) if ever omitted, so a
    // shape surprise never paints a false "unlimited".
    plan: String(w.plan ?? 'free'),
    seats: numOrNull(w.seats),
    override: normalizeAdminOverride(w.override),
    subscription_status: strOrNull(w.subscription_status)
  };
}

function normalizeAdminUser(raw: unknown): AdminUser {
  const u = (raw ?? {}) as Record<string, unknown>;
  return {
    id: String(u.id ?? ''),
    email: String(u.email ?? ''),
    display_name: String(u.display_name ?? u.name ?? ''),
    created_at: String(u.created_at ?? ''),
    is_admin: Boolean(u.is_admin),
    workspaces: num(u.workspaces)
  };
}

function normalizeAdminShare(raw: unknown): AdminShare {
  const s = (raw ?? {}) as Record<string, unknown>;
  return {
    id: String(s.id ?? ''),
    slug: String(s.slug ?? ''),
    url: strOrNull(s.url),
    workspace_slug: String(s.workspace_slug ?? ''),
    game: String(s.game ?? ''),
    sessions_count: num(s.sessions_count),
    spins_count: num(s.spins_count),
    revoked_at: strOrNull(s.revoked_at),
    created_at: String(s.created_at ?? '')
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

/**
 * Share-link label rule (a subdomain label): `^[a-z0-9][a-z0-9-]{0,38}[a-z0-9]$`
 * — 2–40 chars, lowercase alphanumerics + hyphens, no leading/trailing hyphen.
 * Looser than `SLUG_RE` (share labels may be as short as 2 chars).
 */
export const SHARE_SLUG_RE = /^[a-z0-9][a-z0-9-]{0,38}[a-z0-9]$/;

export function isValidShareSlug(slug: string): boolean {
  return SHARE_SLUG_RE.test(slug);
}

/**
 * Custom play domain rule — mirrors the server's `validate_custom_domain`: a
 * lowercase DNS name of ≥ 2 labels, each 1-63 chars of `[a-z0-9-]` with no
 * leading/trailing hyphen, total ≤ 253, and not an IPv4 literal. The server is
 * authoritative (it also rejects domains overlapping this platform's own host);
 * this is just the inline UI check.
 */
const DOMAIN_LABEL = '[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?';
export const PLAY_DOMAIN_RE = new RegExp(`^(?:${DOMAIN_LABEL}\\.)+${DOMAIN_LABEL}$`);
const IPV4_RE = /^\d{1,3}(?:\.\d{1,3}){3}$/;

export function isValidPlayDomain(domain: string): boolean {
  const d = domain.trim().toLowerCase();
  return d.length > 0 && d.length <= 253 && !IPV4_RE.test(d) && PLAY_DOMAIN_RE.test(d);
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
      return {
        password: r.password ?? true,
        github: r.github ?? false,
        discord: r.discord ?? false
      };
    },
    /** Full-page navigation target for GitHub OAuth (never fetched). */
    githubStartUrl(): string {
      return `${BASE}/auth/github/start`;
    },
    /** Full-page navigation target for Discord OAuth (never fetched). */
    discordStartUrl(): string {
      return `${BASE}/auth/discord/start`;
    },
    /**
     * Request a password-reset email. Always resolves (the server returns a
     * uniform 200 whether or not the account exists), so callers show the same
     * "check your inbox" confirmation regardless.
     */
    async forgotPassword(email: string): Promise<void> {
      await request<void>('POST', '/auth/forgot-password', { email });
    },
    /**
     * Redeem a reset token and set a new password. Throws ApiError with code
     * `invalid_token` (400) for an expired/used/unknown token, or `weak_password`
     * (422) for a password under 8 characters.
     */
    async resetPassword(token: string, password: string): Promise<void> {
      await request<void>('POST', '/auth/reset-password', { token, password });
    },
    /**
     * Redeem an email-verification token. Throws ApiError `invalid_token` (400)
     * for an expired/used/unknown token.
     */
    async verifyEmail(token: string): Promise<void> {
      await request<void>('POST', '/auth/verify-email', { token });
    },
    /** Session-auth: resend the verification email. No-op 200 when already verified. */
    async resendVerification(): Promise<void> {
      await request<void>('POST', '/auth/resend-verification');
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
        members: membersRaw.map(normalizeMember),
        custom_play_domain: strOrNull(raw?.custom_play_domain)
      };
    },
    /**
     * Owner-only: attach a custom play domain (or clear it with `null`). Returns
     * the stored value (lowercased server-side). Throws ApiError on 409
     * `domain_taken` or 422 `invalid_domain`.
     */
    async setDomain(slug: string, domain: string | null): Promise<string | null> {
      const raw = await request<unknown>('PUT', `/workspaces/${encodeURIComponent(slug)}/domain`, {
        domain
      });
      const r = (raw ?? {}) as Record<string, unknown>;
      return strOrNull(r.domain);
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
    },

    // ---- Front bundles (M5) -----------------------------------------------
    // A share serves the game's front build. Bundles content-address exactly
    // like math: `frontCheck` learns the missing hashes, blobs upload through
    // the SAME `putBlob` (`PUT …/blobs/:hash`), then `frontCommit` stores the
    // manifest. Membership + `push:math` (the session's `full` scope) authorizes
    // it, so the browser calls these directly.

    /** Front-bundle sibling of `check`: which of a build's blobs are still missing. */
    async frontCheck(slug: string, game: string, files: RevisionFile[]): Promise<string[]> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/front-bundles/check`,
        { files }
      );
      return normalizeHashList(raw);
    },

    /**
     * Commit a front bundle from an already-uploaded manifest (root `index.html`,
     * ≤ 2000 files). Returns the new bundle's id + created_at. Throws ApiError on
     * 409 `missing_blobs` (its `.details.missing` lists hashes to re-upload) or
     * 422 `invalid_manifest`.
     */
    async frontCommit(slug: string, game: string, files: RevisionFile[]): Promise<FrontBundleCreated> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/front-bundles`,
        { files }
      );
      const r = (raw ?? {}) as Record<string, unknown>;
      return { id: String(r.id ?? ''), created_at: String(r.created_at ?? '') };
    },

    // ---- Content lifecycle (delete to free storage) -----------------------

    /** List a game's front bundles (newest first, cap 50) with derived sizes. */
    async frontBundles(slug: string, game: string): Promise<FrontBundleSummary[]> {
      const raw = await request<unknown>(
        'GET',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/front-bundles`
      );
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { bundles?: unknown[] } | undefined)?.bundles ?? []);
      return arr.map(normalizeFrontBundleSummary);
    },

    /**
     * Delete a revision (owner/admin) and GC its now-unreferenced blobs. Returns
     * the freed storage. Throws ApiError on 409 `revision_pinned` (its `.message`
     * lists the pinning share slugs) — surface that to the user.
     */
    async deleteRevision(
      slug: string,
      game: string,
      number: number | string
    ): Promise<DeletionResult> {
      const raw = await request<unknown>(
        'DELETE',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/revisions/${encodeURIComponent(String(number))}`
      );
      return normalizeDeletionResult(raw);
    },

    /**
     * Delete a front bundle (owner/admin) and GC its now-unreferenced blobs.
     * Returns the freed storage. Throws ApiError on 409 `bundle_pinned` (message
     * lists the pinning share slugs) or `last_bundle`.
     */
    async deleteFrontBundle(slug: string, game: string, id: string): Promise<DeletionResult> {
      const raw = await request<unknown>(
        'DELETE',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/front-bundles/${encodeURIComponent(id)}`
      );
      return normalizeDeletionResult(raw);
    }
  },

  shares: {
    /** List a game's share links (newest first) with their counters. */
    async list(slug: string, game: string): Promise<ShareLink[]> {
      const raw = await request<unknown>(
        'GET',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/shares`
      );
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { shares?: unknown[] } | undefined)?.shares ?? []);
      return arr.map(normalizeShareLink);
    },

    /**
     * Create a share link (owner/admin). A 403 `upgrade_required` is thrown when
     * the plan's active-link quota is reached (surface via `isUpgradeError`).
     */
    async create(slug: string, game: string, input: CreateShareInput = {}): Promise<ShareLink> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/shares`,
        input
      );
      return normalizeShareLink(raw);
    },

    /**
     * Patch a share link (owner/admin). Only the keys present in `patch` change;
     * see `UpdateShareInput` for the absent-vs-null tri-state rules.
     */
    async update(
      slug: string,
      game: string,
      id: string,
      patch: UpdateShareInput
    ): Promise<ShareLink> {
      const raw = await request<unknown>(
        'PATCH',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/shares/${encodeURIComponent(id)}`,
        patch
      );
      return normalizeShareLink(raw);
    },

    /** Revoke a share link (a convenience `update` with `revoked: true`). */
    async revoke(slug: string, game: string, id: string): Promise<ShareLink> {
      const raw = await request<unknown>(
        'PATCH',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/shares/${encodeURIComponent(id)}`,
        { revoked: true } satisfies UpdateShareInput
      );
      return normalizeShareLink(raw);
    },

    /** Permanently delete a share link. */
    async remove(slug: string, game: string, id: string): Promise<void> {
      await request<void>(
        'DELETE',
        `/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}/shares/${encodeURIComponent(id)}`
      );
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
     * Owner-only: start a Stripe checkout for the seat subscription. `seats`
     * (1..=100) is the subscription quantity, priced by Stripe's graduated tiers
     * (€3 first seat + €2 each additional). `storageUnits` (0..=100, default 0)
     * bundles the storage add-on into the SAME checkout as a second line item
     * (one unit = +10 GiB for €1/mo); 0 omits it. Returns the hosted checkout URL
     * to navigate to (`window.location.href = url`). 404s when billing is
     * disabled; throws an ApiError `invalid_seats` / `invalid_units` when a count
     * is out of range.
     */
    async checkout(
      slug: string,
      interval: BillingInterval,
      seats: number,
      storageUnits = 0
    ): Promise<string> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/billing/checkout`,
        { interval, seats, storage_units: storageUnits }
      );
      const r = (raw ?? {}) as Record<string, unknown>;
      return String(r.checkout_url ?? '');
    },
    /**
     * Owner-only: change the seat count on an already-subscribed workspace.
     * `seats` (1..=100) becomes the subscription quantity; Stripe prorates the
     * change (`create_prorations`) so only the difference is billed on the next
     * invoice. Returns the fresh billing status. Throws an ApiError `invalid_seats`
     * (out of range), `no_subscription` (no active seat subscription to change),
     * or `seats_below_members` (fewer seats than current members).
     */
    async updateSeats(slug: string, seats: number): Promise<BillingStatus> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/billing/seats`,
        { seats }
      );
      return normalizeBillingStatus(raw);
    },
    /**
     * Owner-only: start a Stripe checkout for the storage add-on. `units` (1..=100)
     * is the line-item quantity, each unit granting +10 GiB for €1/mo. Returns the
     * hosted checkout URL to navigate to. 404s when billing is disabled; throws an
     * ApiError with code `invalid_units` when `units` is out of range.
     */
    async buyStorage(slug: string, units: number): Promise<string> {
      const raw = await request<unknown>(
        'POST',
        `/workspaces/${encodeURIComponent(slug)}/billing/storage`,
        { units }
      );
      const r = (raw ?? {}) as Record<string, unknown>;
      return String(r.checkout_url ?? '');
    }
  },

  admin: {
    /**
     * Probe whether the current session is an instance admin. Non-admins get a
     * flat 404 here (as on every /admin endpoint), which we resolve to `false`
     * WITHOUT throwing — the caller gates UI on a boolean, never an error. Any
     * OTHER failure (network, 5xx) rejects, so the module-cache probe in
     * `admin.ts` can evict and re-try rather than caching a transient miss.
     */
    async me(): Promise<boolean> {
      try {
        const raw = await request<unknown>('GET', '/admin/me');
        const r = (raw ?? {}) as Record<string, unknown>;
        return Boolean(r.is_admin);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return false;
        throw e;
      }
    },

    /** Instance-wide totals + the two 30-day activity series. */
    async overview(): Promise<AdminOverview> {
      const raw = await request<unknown>('GET', '/admin/overview');
      return normalizeAdminOverview(raw);
    },

    /** Workspaces (optionally filtered by `query` — slug/name substring). */
    async workspaces(query = ''): Promise<AdminWorkspace[]> {
      const q = query.trim() ? `?query=${encodeURIComponent(query.trim())}` : '';
      const raw = await request<unknown>('GET', `/admin/workspaces${q}`);
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { workspaces?: unknown[] } | undefined)?.workspaces ?? []);
      return arr.map(normalizeAdminWorkspace);
    },

    /**
     * Set (or clear, with `plan: null`) a workspace's plan override — the
     * comp-subscription grant. Returns the updated row.
     */
    async setOverride(id: string, input: AdminOverrideInput): Promise<AdminWorkspace> {
      const raw = await request<unknown>(
        'PUT',
        `/admin/workspaces/${encodeURIComponent(id)}/override`,
        input
      );
      const r = (raw ?? {}) as Record<string, unknown>;
      return normalizeAdminWorkspace(r.workspace ?? r);
    },

    /** Users (optionally filtered by `query` — email/name substring). */
    async users(query = ''): Promise<AdminUser[]> {
      const q = query.trim() ? `?query=${encodeURIComponent(query.trim())}` : '';
      const raw = await request<unknown>('GET', `/admin/users${q}`);
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { users?: unknown[] } | undefined)?.users ?? []);
      return arr.map(normalizeAdminUser);
    },

    /**
     * Grant/revoke instance admin on a user. Returns the resulting `is_admin`.
     * A 409 `last_admin` (refusing to remove the final admin) surfaces as an
     * `ApiError` the caller classifies by `code`.
     */
    async setAdmin(id: string, is_admin: boolean): Promise<boolean> {
      const raw = await request<unknown>('PUT', `/admin/users/${encodeURIComponent(id)}/admin`, {
        is_admin
      });
      const r = (raw ?? {}) as Record<string, unknown>;
      return Boolean(r.is_admin);
    },

    /** Share links across all workspaces (optionally filtered by `query`). */
    async shares(query = ''): Promise<AdminShare[]> {
      const q = query.trim() ? `?query=${encodeURIComponent(query.trim())}` : '';
      const raw = await request<unknown>('GET', `/admin/shares${q}`);
      const arr = Array.isArray(raw)
        ? raw
        : ((raw as { shares?: unknown[] } | undefined)?.shares ?? []);
      return arr.map(normalizeAdminShare);
    },

    /** Revoke a share link (moderation). Resolves on 200. */
    async revokeShare(id: string): Promise<void> {
      await request<void>('POST', `/admin/shares/${encodeURIComponent(id)}/revoke`);
    }
  },

  device: {
    /** Approve or deny a desktop/CLI device-authorization request (requires auth). */
    async approve(user_code: string, approve: boolean): Promise<void> {
      await request<void>('POST', '/auth/device/approve', { user_code, approve });
    }
  }
};
