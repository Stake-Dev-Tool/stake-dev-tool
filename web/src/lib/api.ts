/**
 * web/src/lib/api.ts
 *
 * Hand-written typed client against the M1 API contract (identity & workspaces).
 * This is the ONLY place that talks to the network — every page/component imports
 * types and calls from here, so when field names shift at integration only this
 * file changes.
 *
 * TO BE RECONCILED at integration with the generated bindings in
 * `ui/src/lib/protocol` (ts-rs output from `crates/protocol`). Until the server
 * is finalized, the shapes below are the source of truth for the dashboard and
 * are parsed defensively (see the `normalize*` helpers) so a nested-vs-flat
 * response difference doesn't ripple into the UI.
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
  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

/** True when the failure is specifically an unauthenticated one (redirect to login). */
export function isUnauthorized(e: unknown): boolean {
  return e instanceof ApiError && e.status === 401;
}

// ---------------------------------------------------------------------------
// Core request helper
// ---------------------------------------------------------------------------

const BASE = '/api';

type Method = 'GET' | 'POST' | 'PATCH' | 'DELETE' | 'PUT';

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

  const raw = await res.text();
  let data: unknown = undefined;
  if (raw) {
    try {
      data = JSON.parse(raw);
    } catch {
      data = undefined;
    }
  }

  if (!res.ok) {
    const err = (data as { error?: { code?: string; message?: string } } | undefined)?.error;
    throw new ApiError(
      res.status,
      err?.code ?? `http_${res.status}`,
      err?.message ?? (res.statusText || 'Request failed')
    );
  }

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
          max_uses: i.max_uses == null ? null : Number(i.max_uses),
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
        max_uses: info.max_uses == null ? null : Number(info.max_uses)
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
     * Accept an invite (requires auth). The membership shape isn't guaranteed to
     * carry the slug, so we return a best-effort membership and callers fall back
     * to re-listing workspaces if `workspace.slug` is empty.
     */
    async accept(token: string): Promise<WorkspaceMembership> {
      const r = await request<unknown>('POST', '/invites/accept', { token });
      return normalizeMembership(r);
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

  device: {
    /** Approve or deny a desktop/CLI device-authorization request (requires auth). */
    async approve(user_code: string, approve: boolean): Promise<void> {
      await request<void>('POST', '/auth/device/approve', { user_code, approve });
    }
  }
};
