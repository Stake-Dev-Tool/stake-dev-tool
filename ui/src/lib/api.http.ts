// Tauri-free API surface.
//
// Everything in this module talks to the LGS over plain HTTP/SSE (or is a pure
// constant/helper) and therefore carries **no** `@tauri-apps` import. It is the
// only api module the `/test` route (and any future cloud/share workbench) is
// allowed to reach, so the resulting chunk stays Tauri-free.
//
// The HTTP/SSE/prepare surface is parameterised on `apiBase` (via
// `createDevtoolClient`) instead of hardcoding same-origin paths: `''` reproduces
// today's desktop same-origin behavior; a cloud/share prefix re-bases every call.

// ===== Shared data types (also consumed by the Tauri clients) =====

export type ResolutionPreset = {
  id: string;
  label: string;
  width: number;
  height: number;
  enabled: boolean;
  builtin: boolean;
};

export type Settings = { resolutions: ResolutionPreset[] };

export type PrepareSession = {
  sessionId: string;
  gameSlug: string;
  balance?: number;
  currency?: string;
  language?: string;
};

// ---- Force event / last event / replay ----

export type ForcedEvent = { mode: string; eventId: number };
export type ForcedEventStatus = { forced: ForcedEvent | null };
export type LastEvent = { eventId: number | null; payoutMultiplier: number | null };

export type SavedRound = {
  id: string;
  gameSlug: string;
  mode: string;
  eventId: number;
  description: string;
  createdAt: number;
  updatedAt: number;
};

export type EventEntry = {
  eventId: number;
  mode: string;
  betAmount: number;
  payout: number;
  payoutMultiplier: number;
  forced: boolean;
  at: number;
};

export type EventsHistory = { count: number; events: EventEntry[] };

// ---- Notable bets per mode (computed from the lookup table) ----

export type NotableBet = { eventId: number; payoutMultiplier: number };
export type NotableBucket = 'zero' | 'low' | 'medium' | 'big' | 'max';
export type BetStats = Record<NotableBucket, NotableBet[]>;
export type ModeBetStats = { mode: string; stats: BetStats };

// ===== Devtool HTTP/SSE client =====
//
// `apiBase` is prepended to every path. Desktop passes `''` so the requests stay
// same-origin relative paths (`/api/devtool/…`) — byte-identical to the previous
// hardcoded clients. A cloud/share workbench passes a scoped prefix.
export function createDevtoolClient(apiBase = '') {
  return {
    settings: {
      get: async (): Promise<Settings> => {
        const r = await fetch(`${apiBase}/api/devtool/settings`);
        if (!r.ok) throw new Error(`get_settings: ${r.status}`);
        return r.json();
      },
      toggle: async (id: string, enabled: boolean): Promise<Settings> => {
        const r = await fetch(`${apiBase}/api/devtool/settings/toggle`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ id, enabled })
        });
        if (!r.ok) throw new Error(`toggle: ${r.status}`);
        return r.json();
      },
      addCustom: async (label: string, width: number, height: number): Promise<Settings> => {
        const r = await fetch(`${apiBase}/api/devtool/settings/custom`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ label, width, height })
        });
        if (!r.ok) {
          const t = await r.text();
          throw new Error(`addCustom: ${r.status} ${t}`);
        }
        return r.json();
      },
      deleteCustom: async (id: string): Promise<Settings> => {
        const r = await fetch(`${apiBase}/api/devtool/settings/custom/${encodeURIComponent(id)}`, {
          method: 'DELETE'
        });
        if (!r.ok) throw new Error(`deleteCustom: ${r.status}`);
        return r.json();
      }
    },

    forcedEvent: {
      get: async (): Promise<ForcedEventStatus> => {
        const r = await fetch(`${apiBase}/api/devtool/force-event`);
        if (!r.ok) throw new Error(`get force-event: ${r.status}`);
        return r.json();
      },
      set: async (mode: string, eventId: number): Promise<ForcedEventStatus> => {
        const r = await fetch(`${apiBase}/api/devtool/force-event`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ mode, eventId })
        });
        if (!r.ok) {
          const t = await r.text();
          throw new Error(`set force-event: ${r.status} ${t}`);
        }
        return r.json();
      },
      clear: async (): Promise<ForcedEventStatus> => {
        const r = await fetch(`${apiBase}/api/devtool/force-event`, { method: 'DELETE' });
        if (!r.ok) throw new Error(`clear force-event: ${r.status}`);
        return r.json();
      }
    },

    savedRounds: {
      list: async (gameSlug?: string): Promise<SavedRound[]> => {
        const qs = gameSlug ? `?gameSlug=${encodeURIComponent(gameSlug)}` : '';
        const r = await fetch(`${apiBase}/api/devtool/saved-rounds${qs}`);
        if (!r.ok) throw new Error(`list saved-rounds: ${r.status}`);
        const j = (await r.json()) as { rounds: SavedRound[] };
        return j.rounds;
      },
      create: async (
        gameSlug: string,
        mode: string,
        eventId: number,
        description: string
      ): Promise<SavedRound> => {
        const r = await fetch(`${apiBase}/api/devtool/saved-rounds`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ gameSlug, mode, eventId, description })
        });
        if (!r.ok) {
          const t = await r.text();
          throw new Error(`create saved-round: ${r.status} ${t}`);
        }
        return r.json();
      },
      update: async (id: string, description: string): Promise<SavedRound> => {
        const r = await fetch(`${apiBase}/api/devtool/saved-rounds/${encodeURIComponent(id)}`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ description })
        });
        if (!r.ok) {
          const t = await r.text();
          throw new Error(`update saved-round: ${r.status} ${t}`);
        }
        return r.json();
      },
      remove: async (id: string): Promise<void> => {
        const r = await fetch(`${apiBase}/api/devtool/saved-rounds/${encodeURIComponent(id)}`, {
          method: 'DELETE'
        });
        if (!r.ok) throw new Error(`delete saved-round: ${r.status}`);
      }
    },

    lastEvent: {
      get: async (sessionId: string): Promise<LastEvent> => {
        const r = await fetch(
          `${apiBase}/api/devtool/sessions/${encodeURIComponent(sessionId)}/last-event`
        );
        if (!r.ok) throw new Error(`last-event: ${r.status}`);
        return r.json();
      }
    },

    history: {
      get: async (sessionId: string): Promise<EventsHistory> => {
        const r = await fetch(
          `${apiBase}/api/devtool/sessions/${encodeURIComponent(sessionId)}/events`
        );
        if (!r.ok) throw new Error(`events history: ${r.status}`);
        return r.json();
      }
    },

    sessions: {
      reset: async (): Promise<void> => {
        const r = await fetch(`${apiBase}/api/devtool/sessions`, { method: 'DELETE' });
        if (!r.ok) {
          const t = await r.text();
          throw new Error(`reset sessions: ${r.status} ${t}`);
        }
      }
    },

    betStats: {
      get: async (gameSlug: string): Promise<ModeBetStats[]> => {
        const r = await fetch(`${apiBase}/api/devtool/bet-stats/${encodeURIComponent(gameSlug)}`);
        if (!r.ok) throw new Error(`bet-stats: ${r.status}`);
        const j = (await r.json()) as { modes: ModeBetStats[] };
        return j.modes;
      }
    },

    gameModes: {
      get: async (gameSlug: string): Promise<string[]> => {
        const r = await fetch(`${apiBase}/api/devtool/games/${encodeURIComponent(gameSlug)}/modes`);
        if (!r.ok) throw new Error(`game modes: ${r.status}`);
        const j = (await r.json()) as { modes: string[] };
        return j.modes;
      }
    },

    /** POST the session bootstrap (balance/currency/language) before loading a frame. */
    prepareSession: async (payload: PrepareSession): Promise<void> => {
      const res = await fetch(`${apiBase}/api/devtool/sessions/prepare`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload)
      });
      if (!res.ok) {
        const text = await res.text();
        throw new Error(`prepare failed: ${res.status} ${text}`);
      }
    },

    /** SSE endpoint for a session's live event stream. */
    streamUrl: (sessionId: string): string =>
      `${apiBase}/api/devtool/sessions/${encodeURIComponent(sessionId)}/stream`
  };
}

export type DevtoolClient = ReturnType<typeof createDevtoolClient>;

// ===== Cloud workbench client (test view "Versions" + push notifications) =====
//
// Unlike `createDevtoolClient`, these endpoints do NOT live under the tenant
// `apiBase` prefix — the dashboard API and the workspace SSE stream are mounted
// at the origin root: `/api/workspaces/:ws/…`. Same-origin, cookie auth. Only
// the cloud test view (when `ctx.auth.workspace` is set) reaches this; the
// desktop context never constructs the client at all.

/** A revision as listed under `GET .../revisions` (newest first). Mirrors the
 *  server's `RevisionSummary`; every field past `number` is treated optional so
 *  the picker degrades if the shape narrows. (Distinct from the desktop
 *  `CloudRevisionSummary` in `api.tauri.ts` — this is the Tauri-free subset the
 *  test view needs.) */
export type WorkbenchRevision = {
  number: number;
  message?: string;
  created_at?: string;
  stats_status?: string | null;
};

/** A front bundle as listed under `GET .../front-bundles` (newest first). */
export type WorkbenchFrontBundle = {
  id: string;
  created_at?: string;
  files_count?: number;
  total_size?: number;
};

/** Error carrying the HTTP status so callers can special-case a 404 (the new
 *  endpoint is not deployed yet) and leave the feature dormant. */
export class CloudHttpError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'CloudHttpError';
    this.status = status;
  }
}

export function createCloudWorkbenchClient(workspaceSlug: string) {
  // `workspaceSlug` is already a URL path segment (extracted from the tenant
  // prefix) so it is used verbatim; the game slug arrives decoded and is
  // encoded here.
  const ws = workspaceSlug;
  return {
    /** `GET /api/workspaces/:ws/games/:game/revisions` — newest first (exists today). */
    listRevisions: async (game: string): Promise<WorkbenchRevision[]> => {
      const r = await fetch(`/api/workspaces/${ws}/games/${encodeURIComponent(game)}/revisions`, {
        credentials: 'same-origin'
      });
      if (!r.ok) throw new CloudHttpError(r.status, `list revisions: ${r.status}`);
      const j = (await r.json()) as { revisions: WorkbenchRevision[] };
      return j.revisions ?? [];
    },
    /** `GET /api/workspaces/:ws/games/:game/front-bundles` — newest first (NEW;
     *  may 404 until deployed → the caller then hides the bundle options). */
    listFrontBundles: async (game: string): Promise<WorkbenchFrontBundle[]> => {
      const r = await fetch(
        `/api/workspaces/${ws}/games/${encodeURIComponent(game)}/front-bundles`,
        { credentials: 'same-origin' }
      );
      if (!r.ok) throw new CloudHttpError(r.status, `list front-bundles: ${r.status}`);
      const j = (await r.json()) as { bundles: WorkbenchFrontBundle[] };
      return j.bundles ?? [];
    },
    /** Workspace SSE stream URL — carries named `revision_pushed` / `front_pushed`
     *  events. Consumed via `EventSource` (same-origin cookie). */
    eventsUrl: (): string => `/api/workspaces/${ws}/events`
  };
}

export type CloudWorkbenchClient = ReturnType<typeof createCloudWorkbenchClient>;

export function replayUrl(
  gameUrl: string,
  gameSlug: string,
  lgsHostPort: string,
  opts: {
    mode: string;
    eventId: number;
    version?: string;
    currency?: string;
    amount?: number;
    lang?: string;
    device?: string;
    social?: boolean;
  }
): string {
  const u = new URL(gameUrl);
  u.searchParams.set('replay', 'true');
  u.searchParams.set('game', gameSlug);
  u.searchParams.set('version', opts.version ?? '1');
  u.searchParams.set('mode', opts.mode);
  u.searchParams.set('event', String(opts.eventId));
  u.searchParams.set('rgs_url', lgsHostPort);
  if (opts.currency) u.searchParams.set('currency', opts.currency);
  if (opts.amount !== undefined) u.searchParams.set('amount', String(opts.amount));
  if (opts.lang) u.searchParams.set('lang', opts.lang);
  if (opts.device) u.searchParams.set('device', opts.device);
  if (opts.social !== undefined) u.searchParams.set('social', opts.social ? 'true' : 'false');
  return u.toString();
}

export type Resolution = {
  id: string;
  label: string;
  width: number;
  height: number;
};

export const RESOLUTIONS: Resolution[] = [
  { id: 'desktop', label: 'Desktop', width: 1200, height: 675 },
  { id: 'laptop', label: 'Laptop', width: 1024, height: 576 },
  { id: 'popout-l', label: 'Popout L', width: 800, height: 450 },
  { id: 'popout-s', label: 'Popout S', width: 400, height: 225 },
  { id: 'mobile-l', label: 'Mobile L', width: 425, height: 821 },
  { id: 'mobile-m', label: 'Mobile M', width: 375, height: 667 },
  { id: 'mobile-s', label: 'Mobile S', width: 320, height: 568 }
];

// `country` is an ISO 3166-1 alpha-2 code used to fetch a flag SVG/PNG from
// flagcdn.com. `null` means no flag (fallback icon shown instead).
export type LanguageInfo = { code: string; name: string; country: string | null };
export type CurrencyInfo = {
  code: string;
  name: string;
  symbol: string;
  country: string | null;
  /** optional emoji/text fallback when no country flag fits (e.g. social tokens) */
  badge?: string;
};

export const LANGUAGES: LanguageInfo[] = [
  { code: 'ar', name: 'Arabic',     country: 'sa' },
  { code: 'de', name: 'German',     country: 'de' },
  { code: 'en', name: 'English',    country: 'gb' },
  { code: 'es', name: 'Spanish',    country: 'es' },
  { code: 'fi', name: 'Finnish',    country: 'fi' },
  { code: 'fr', name: 'French',     country: 'fr' },
  { code: 'hi', name: 'Hindi',      country: 'in' },
  { code: 'id', name: 'Indonesian', country: 'id' },
  { code: 'ja', name: 'Japanese',   country: 'jp' },
  { code: 'ko', name: 'Korean',     country: 'kr' },
  { code: 'pl', name: 'Polish',     country: 'pl' },
  { code: 'pt', name: 'Portuguese', country: 'pt' },
  { code: 'ru', name: 'Russian',    country: 'ru' },
  { code: 'tr', name: 'Turkish',    country: 'tr' },
  { code: 'vi', name: 'Vietnamese', country: 'vn' },
  { code: 'zh', name: 'Chinese',    country: 'cn' }
];

export const CURRENCIES: CurrencyInfo[] = [
  { code: 'USD', name: 'United States Dollar',       symbol: '$',    country: 'us' },
  { code: 'CAD', name: 'Canadian Dollar',            symbol: 'CA$',  country: 'ca' },
  { code: 'JPY', name: 'Japanese Yen',               symbol: '¥',    country: 'jp' },
  { code: 'EUR', name: 'Euro',                       symbol: '€',    country: 'eu' },
  { code: 'RUB', name: 'Russian Ruble',              symbol: '₽',    country: 'ru' },
  { code: 'CNY', name: 'Chinese Yuan',               symbol: 'CN¥',  country: 'cn' },
  { code: 'PHP', name: 'Philippine Peso',            symbol: '₱',    country: 'ph' },
  { code: 'INR', name: 'Indian Rupee',               symbol: '₹',    country: 'in' },
  { code: 'IDR', name: 'Indonesian Rupiah',          symbol: 'Rp',   country: 'id' },
  { code: 'KRW', name: 'South Korean Won',           symbol: '₩',    country: 'kr' },
  { code: 'BRL', name: 'Brazilian Real',             symbol: 'R$',   country: 'br' },
  { code: 'MXN', name: 'Mexican Peso',               symbol: 'MX$',  country: 'mx' },
  { code: 'DKK', name: 'Danish Krone',               symbol: 'KR',   country: 'dk' },
  { code: 'PLN', name: 'Polish Złoty',               symbol: 'zł',   country: 'pl' },
  { code: 'VND', name: 'Vietnamese Đồng',            symbol: '₫',    country: 'vn' },
  { code: 'TRY', name: 'Turkish Lira',               symbol: '₺',    country: 'tr' },
  { code: 'CLP', name: 'Chilean Peso',               symbol: 'CLP',  country: 'cl' },
  { code: 'ARS', name: 'Argentine Peso',             symbol: 'ARS',  country: 'ar' },
  { code: 'PEN', name: 'Peruvian Sol',               symbol: 'S/',   country: 'pe' },
  { code: 'NGN', name: 'Nigerian Naira',             symbol: '₦',    country: 'ng' },
  { code: 'SAR', name: 'Saudi Arabia Riyal',         symbol: 'SAR',  country: 'sa' },
  { code: 'ILS', name: 'Israel Shekel',              symbol: 'ILS',  country: 'il' },
  { code: 'AED', name: 'United Arab Emirates Dirham', symbol: 'AED', country: 'ae' },
  { code: 'TWD', name: 'Taiwan New Dollar',          symbol: 'NT$',  country: 'tw' },
  { code: 'NOK', name: 'Norway Krone',               symbol: 'kr',   country: 'no' },
  { code: 'KWD', name: 'Kuwaiti Dinar',              symbol: 'KD',   country: 'kw' },
  { code: 'JOD', name: 'Jordanian Dinar',            symbol: 'JD',   country: 'jo' },
  { code: 'CRC', name: 'Costa Rica Colon',           symbol: '₡',    country: 'cr' },
  { code: 'TND', name: 'Tunisian Dinar',             symbol: 'TND',  country: 'tn' },
  { code: 'SGD', name: 'Singapore Dollar',           symbol: 'SG$',  country: 'sg' },
  { code: 'MYR', name: 'Malaysia Ringgit',           symbol: 'RM',   country: 'my' },
  { code: 'OMR', name: 'Oman Rial',                  symbol: 'OMR',  country: 'om' },
  { code: 'QAR', name: 'Qatar Riyal',                symbol: 'QAR',  country: 'qa' },
  { code: 'BHD', name: 'Bahraini Dinar',             symbol: 'BD',   country: 'bh' },
  { code: 'XGC', name: 'Stake Gold Coin',            symbol: 'GC',   country: null, badge: 'GC' },
  { code: 'XSC', name: 'Stake Cash (US)',            symbol: 'SC',   country: null, badge: 'SC' },
  { code: 'XEC', name: 'Stake Cash (EU)',            symbol: 'SC',   country: null, badge: 'SC' }
];

export function flagUrl(country: string | null | undefined, height = 20): string | null {
  if (!country) return null;
  return `https://flagcdn.com/h${height}/${country}.png`;
}

export const API_MULTIPLIER = 1_000_000;
