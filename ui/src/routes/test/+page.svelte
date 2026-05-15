<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { page } from '$app/state';
  import Picker from '$lib/Picker.svelte';
  import {
    LANGUAGES,
    CURRENCIES,
    API_MULTIPLIER,
    settingsHttp,
    forcedEventHttp,
    savedRoundsHttp,
    sessionsHttp,
    betStatsHttp,
    gameModesHttp,
    replayUrl,
    type ResolutionPreset,
    type EventEntry,
    type SavedRound,
    type ModeBetStats,
    type NotableBucket
  } from '$lib/api';

  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { Switch } from '$lib/components/ui/switch';
  import { Badge } from '$lib/components/ui/badge';
  import { Separator } from '$lib/components/ui/separator';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import * as Dialog from '$lib/components/ui/dialog';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { Toaster } from '$lib/components/ui/sonner';
  import { toast } from 'svelte-sonner';

  import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
  import VolumeIcon from '@lucide/svelte/icons/volume-2';
  import VolumeOffIcon from '@lucide/svelte/icons/volume-x';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import StarIcon from '@lucide/svelte/icons/star';
  import StarOffIcon from '@lucide/svelte/icons/star-off';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import LayoutIcon from '@lucide/svelte/icons/layout-grid';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import XIcon from '@lucide/svelte/icons/x';
  import RewindIcon from '@lucide/svelte/icons/rewind';
  import ZapIcon from '@lucide/svelte/icons/zap';
  import HistoryIcon from '@lucide/svelte/icons/history';

  // Stake social-mode currencies (XGC = Gold Coin, XSC = Stake Cash) only
  // make sense when social=true; hide them otherwise.
  const SOCIAL_CURRENCIES = new Set(['XGC', 'XSC']);
  const realCurrencies = CURRENCIES.filter((c) => !SOCIAL_CURRENCIES.has(c.code));
  const socialCurrencies = CURRENCIES.filter((c) => SOCIAL_CURRENCIES.has(c.code));

  type FrameState = {
    res: ResolutionPreset;
    sessionId: string;
    src: string | null;
    muted: boolean;
    history: EventEntry[];
    showHistory: boolean;
  };

  let gameUrl = $state('');
  let gameSlug = $state('');
  let sidebarCollapsed = $state(false);

  let balance = $state(10000);
  let currency = $state<string>('USD');
  let language = $state<string>('en');
  let social = $state(false);
  let device = $state('desktop');

  // Available currencies depend on social mode: XGC/XSC are social-only.
  const availableCurrencies = $derived(social ? socialCurrencies : realCurrencies);

  // Auto-switch currency when toggling social mode in/out of valid set.
  function toggleSocial(next: boolean) {
    social = next;
    if (social) {
      if (!SOCIAL_CURRENCIES.has(currency)) currency = 'XGC';
    } else {
      if (SOCIAL_CURRENCIES.has(currency)) currency = 'USD';
    }
  }

  let allResolutions = $state<ResolutionPreset[]>([]);
  let frames = $state<FrameState[]>([]);
  let showManage = $state(false);
  let showReplay = $state(false);
  let newCustomLabel = $state('');
  let newCustomWidth = $state(800);
  let newCustomHeight = $state(450);

  function rebuildFramesFromResolutions(prev: FrameState[] = []) {
    const enabled = allResolutions.filter((r) => r.enabled);
    const byId = new Map(prev.map((f) => [f.res.id, f]));
    const persisted = loadPersistedFrameSessionIds();
    frames = enabled.map((res) => {
      const existing = byId.get(res.id);
      if (existing) {
        existing.res = res;
        return existing;
      }
      return {
        res,
        sessionId: persisted[res.id] ?? defaultSessionIdFor(res.id),
        src: null,
        muted: true,
        history: [],
        showHistory: false
      };
    });
    persistFrameSessionIds();
  }

  // Falls back to ['base'] while loading so the dropdowns are never empty.
  let availableModes = $state<string[]>(['base']);

  let forcedMode = $state<string>('base');
  let forcedEventId = $state<number | null>(null);
  let forcedEventBanner = $state<{ mode: string; eventId: number } | null>(null);

  let replayMode = $state<string>('base');
  let replayEventId = $state<number | null>(null);
  let replayAmount = $state(1);

  let savedRounds = $state<SavedRound[]>([]);
  let showSavedRounds = $state(true);
  let savingRound = $state(false);
  let saveDescription = $state('');
  let showSaveInput = $state(false);

  let notableRounds = $state<ModeBetStats[]>([]);
  let notableLoading = $state(false);
  let notableLoaded = $state(false);
  let showNotable = $state(false);
  const notableBuckets: Array<{ kind: NotableBucket; label: string; color: string }> = [
    { kind: 'zero', label: 'zero', color: 'text-muted-foreground' },
    { kind: 'low', label: 'low', color: 'text-sky-400' },
    { kind: 'medium', label: 'med', color: 'text-amber-400' },
    { kind: 'big', label: 'big', color: 'text-orange-400' },
    { kind: 'max', label: 'max', color: 'text-emerald-400' }
  ];

  async function reloadSavedRounds() {
    if (!gameSlug) return;
    try {
      savedRounds = await savedRoundsHttp.list(gameSlug);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }

  async function loadNotableRounds() {
    if (!gameSlug || notableLoading) return;
    notableLoading = true;
    try {
      notableRounds = await betStatsHttp.get(gameSlug);
      notableLoaded = true;
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      notableLoading = false;
    }
  }

  async function toggleNotablePanel() {
    showNotable = !showNotable;
    if (showNotable && !notableLoaded) await loadNotableRounds();
  }

  async function applyForcedFromNotable(mode: string, eventId: number) {
    forcedMode = mode;
    forcedEventId = eventId;
    busy = true;
    try {
      const resp = await forcedEventHttp.set(mode, eventId);
      forcedEventBanner = resp.forced;
      toast.success(`Forced: ${mode} #${eventId}`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  /** Bookmark a notable bet with the auto-set description. */
  async function bookmarkNotable(mode: string, eventId: number, kind: NotableBucket) {
    if (!gameSlug) return;
    if (isBookmarked(mode, eventId)) return;
    const description =
      kind === 'zero'
        ? 'zero win'
        : kind === 'low'
          ? 'low win'
          : kind === 'medium'
            ? 'medium win'
            : kind === 'big'
              ? 'big win'
              : 'max win';
    try {
      await savedRoundsHttp.create(gameSlug, mode, eventId, description);
      await reloadSavedRounds();
      toast.success(`Bookmarked ${mode} #${eventId} (${description})`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }

  async function saveCurrentRound() {
    if (forcedEventId === null || forcedEventId <= 0) {
      toast.error('Enter a valid event id before saving.');
      return;
    }
    if (!gameSlug) return;
    savingRound = true;
    try {
      await savedRoundsHttp.create(
        gameSlug,
        forcedMode,
        forcedEventId,
        saveDescription.trim()
      );
      saveDescription = '';
      showSaveInput = false;
      await reloadSavedRounds();
      toast.success(`Saved ${forcedMode} #${forcedEventId}`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      savingRound = false;
    }
  }

  async function applySavedRound(r: SavedRound) {
    forcedMode = r.mode;
    forcedEventId = r.eventId;
    busy = true;
    try {
      const resp = await forcedEventHttp.set(r.mode, r.eventId);
      forcedEventBanner = resp.forced;
      toast.success(`Forced: ${r.mode} #${r.eventId}${r.description ? ` — ${r.description}` : ''}`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  async function deleteSavedRound(r: SavedRound) {
    if (!confirm(`Delete saved round ${r.mode} #${r.eventId}?`)) return;
    try {
      await savedRoundsHttp.remove(r.id);
      await reloadSavedRounds();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }

  function isBookmarked(mode: string, eventId: number): boolean {
    return savedRounds.some((r) => r.mode === mode && r.eventId === eventId);
  }

  // Modal state for "Bookmark from history" — opens when the user clicks ★ on
  // a history row so they can attach a description before saving.
  let bookmarkModal = $state<{
    mode: string;
    eventId: number;
    description: string;
    saving: boolean;
  } | null>(null);

  function openBookmarkModal(entry: EventEntry) {
    if (isBookmarked(entry.mode, entry.eventId)) return;
    bookmarkModal = {
      mode: entry.mode,
      eventId: entry.eventId,
      description: '',
      saving: false
    };
  }

  function closeBookmarkModal() {
    bookmarkModal = null;
  }

  let bookmarkInputEl = $state<HTMLInputElement | null>(null);
  $effect(() => {
    if (bookmarkModal && bookmarkInputEl) {
      bookmarkInputEl.focus();
    }
  });

  async function confirmBookmark() {
    if (!bookmarkModal || !gameSlug) return;
    bookmarkModal.saving = true;
    try {
      await savedRoundsHttp.create(
        gameSlug,
        bookmarkModal.mode,
        bookmarkModal.eventId,
        bookmarkModal.description.trim()
      );
      await reloadSavedRounds();
      toast.success(`Bookmarked ${bookmarkModal.mode} #${bookmarkModal.eventId}`);
      bookmarkModal = null;
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
      if (bookmarkModal) bookmarkModal.saving = false;
    }
  }

  async function applyForcedEvent() {
    if (forcedEventId === null || forcedEventId <= 0) {
      toast.error('Enter a valid event id.');
      return;
    }
    busy = true;
    try {
      const r = await forcedEventHttp.set(forcedMode, forcedEventId);
      forcedEventBanner = r.forced;
      toast.success(`Forced: ${forcedMode} #${forcedEventId}`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  async function clearForcedEvent() {
    busy = true;
    try {
      await forcedEventHttp.clear();
      forcedEventBanner = null;
      toast.info('Forced event cleared — back to RNG.');
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  async function launchReplay(frame: FrameState) {
    if (replayEventId === null || replayEventId <= 0) {
      toast.error('Enter a valid event id to replay.');
      return;
    }
    if (replayAmount < 0.01 || replayAmount > 1000) {
      toast.error('Replay bet amount must be between 0.01 and 1000.');
      return;
    }
    const url = replayUrl(gameUrl, gameSlug, lgsHostPort, {
      mode: replayMode,
      eventId: replayEventId,
      currency,
      amount: Math.round(replayAmount * API_MULTIPLIER),
      lang: language,
      device,
      social
    });
    frame.src = url;
    toast.success(
      `Replay launched on ${frame.res.label}: ${replayMode} #${replayEventId} at ${replayAmount} ${currency}`
    );
  }

  // One persistent SSE connection per frame (keyed by sessionId). The server
  // emits a `snapshot` event on connect (full current history) then pushes
  // each new event as it happens.
  const eventSources = new Map<string, EventSource>();
  const HISTORY_CAP = 100;

  $effect(() => {
    const activeSids = new Set(frames.map((f) => f.sessionId));

    for (const f of frames) {
      if (eventSources.has(f.sessionId)) continue;
      const frame = f;
      const es = new EventSource(
        `/api/devtool/sessions/${encodeURIComponent(frame.sessionId)}/stream`
      );
      es.addEventListener('snapshot', (ev) => {
        try {
          frame.history = JSON.parse((ev as MessageEvent).data) as EventEntry[];
        } catch {
          // ignore malformed snapshot
        }
      });
      es.addEventListener('event', (ev) => {
        try {
          const entry = JSON.parse((ev as MessageEvent).data) as EventEntry;
          // De-dup: server sends the snapshot then subscribes, so an entry
          // pushed in that window can arrive via both paths.
          const head = frame.history[0];
          if (head && head.at === entry.at && head.eventId === entry.eventId) return;
          const next = [entry, ...frame.history];
          if (next.length > HISTORY_CAP) next.length = HISTORY_CAP;
          frame.history = next;
        } catch {
          // ignore malformed event
        }
      });
      eventSources.set(frame.sessionId, es);
    }

    for (const [sid, es] of eventSources) {
      if (!activeSids.has(sid)) {
        es.close();
        eventSources.delete(sid);
      }
    }
  });

  onDestroy(() => {
    for (const es of eventSources.values()) es.close();
    eventSources.clear();
  });

  function formatAmount(microUnits: number): string {
    return (microUnits / API_MULTIPLIER).toFixed(2);
  }

  let busy = $state(false);

  // We're served by the LGS itself, so APIs are same-origin.
  const lgsBase = `${location.origin}`;
  const lgsHostPort = location.host;
  const SESSION_STORAGE_PREFIX = 'stake-dev-tool:test-sessions:';

  function frameSessionStorageKey(): string | null {
    if (!gameSlug || !gameUrl) return null;
    return `${SESSION_STORAGE_PREFIX}${gameSlug}:${gameUrl}`;
  }

  function defaultSessionIdFor(resolutionId: string): string {
    return `stake-dev-tool:${gameSlug}:${resolutionId}`;
  }

  function loadPersistedFrameSessionIds(): Record<string, string> {
    const key = frameSessionStorageKey();
    if (!key) return {};
    try {
      return JSON.parse(localStorage.getItem(key) ?? '{}') as Record<string, string>;
    } catch {
      return {};
    }
  }

  function persistFrameSessionIds() {
    const key = frameSessionStorageKey();
    if (!key) return;
    const sessionIds = Object.fromEntries(frames.map((f) => [f.res.id, f.sessionId]));
    localStorage.setItem(key, JSON.stringify(sessionIds));
  }

  onMount(async () => {
    const params = page.url.searchParams;
    gameUrl = params.get('gameUrl') ?? '';
    gameSlug = params.get('gameSlug') ?? '';
    if (!gameUrl || !gameSlug) {
      toast.error('Missing game URL or slug. Open this page from the desktop launcher.');
      return;
    }
    try {
      const s = await settingsHttp.get();
      allResolutions = s.resolutions;
      rebuildFramesFromResolutions();
      const f = await forcedEventHttp.get();
      forcedEventBanner = f.forced;
      await reloadSavedRounds();
      await loadGameModes();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
      return;
    }
    await reloadAll();
  });

  /** Read the mode list from the game's index.json. */
  async function loadGameModes() {
    if (!gameSlug) return;
    try {
      const modes = await gameModesHttp.get(gameSlug);
      if (modes.length === 0) return;
      availableModes = modes;
      if (!modes.includes(forcedMode)) forcedMode = modes[0];
      if (!modes.includes(replayMode)) replayMode = modes[0];
    } catch (e) {
      console.warn('failed to load game modes:', e);
    }
  }

  async function toggleResolution(id: string, enabled: boolean) {
    busy = true;
    try {
      const s = await settingsHttp.toggle(id, enabled);
      allResolutions = s.resolutions;
      rebuildFramesFromResolutions(frames);
      const newlyEnabled = frames.filter((f) => f.src === null);
      for (const f of newlyEnabled) {
        await reloadFrame(f);
        await new Promise((r) => setTimeout(r, 600));
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  async function addCustomResolution() {
    const label = newCustomLabel.trim();
    if (!label || newCustomWidth <= 0 || newCustomHeight <= 0) {
      toast.error('Label, width and height required.');
      return;
    }
    busy = true;
    try {
      const s = await settingsHttp.addCustom(label, newCustomWidth, newCustomHeight);
      allResolutions = s.resolutions;
      rebuildFramesFromResolutions(frames);
      newCustomLabel = '';
      const last = frames[frames.length - 1];
      if (last && last.src === null) await reloadFrame(last);
      toast.success(`Added "${label}" (${newCustomWidth}×${newCustomHeight})`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  async function deleteCustomResolution(id: string) {
    if (!confirm('Delete this custom resolution?')) return;
    busy = true;
    try {
      const s = await settingsHttp.deleteCustom(id);
      allResolutions = s.resolutions;
      rebuildFramesFromResolutions(frames);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  function buildGameUrlFor(sessionId: string): string {
    if (!gameUrl) return '';
    const rgsUrl = `${lgsHostPort}/api/rgs/${gameSlug}`;
    const u = new URL(gameUrl);
    u.searchParams.set('sessionID', sessionId);
    u.searchParams.set('rgs_url', rgsUrl);
    u.searchParams.set('lang', language);
    u.searchParams.set('currency', currency);
    u.searchParams.set('device', device);
    u.searchParams.set('social', social ? 'true' : 'false');
    return u.toString();
  }

  async function prepareSession(sessionId: string) {
    const res = await fetch(`${lgsBase}/api/devtool/sessions/prepare`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        sessionId,
        gameSlug,
        balance: Math.round(balance * API_MULTIPLIER),
        currency,
        language
      })
    });
    if (!res.ok) {
      const text = await res.text();
      throw new Error(`prepare failed: ${res.status} ${text}`);
    }
  }

  async function reloadFrame(frame: FrameState, regenerateSession = false) {
    if (regenerateSession) frame.sessionId = crypto.randomUUID();
    persistFrameSessionIds();
    await prepareSession(frame.sessionId);
    frame.src = buildGameUrlFor(frame.sessionId);
  }

  async function reloadAll() {
    busy = true;
    try {
      // Clear all iframes first, then load one at a time to avoid WebGL races.
      frames.forEach((f) => (f.src = null));
      for (const f of frames) {
        await reloadFrame(f);
        await new Promise((r) => setTimeout(r, 800));
      }
      toast.success(`Reloaded ${frames.length} frames · balance=${balance} ${currency}`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  async function resetSessions() {
    if (!confirm('Reset all saved sessions and reload every frame?')) return;
    busy = true;
    try {
      await sessionsHttp.reset();
      frames.forEach((f) => {
        f.sessionId = defaultSessionIdFor(f.res.id);
        f.src = null;
        f.history = [];
        f.showHistory = false;
      });
      persistFrameSessionIds();
      for (const f of frames) {
        await reloadFrame(f);
        await new Promise((r) => setTimeout(r, 800));
      }
      toast.success('Sessions reset.');
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  async function reloadOne(frame: FrameState) {
    busy = true;
    try {
      await reloadFrame(frame);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  function openInBrowser(frame: FrameState) {
    if (!frame.src) return;
    window.open(frame.src, '_blank', 'noopener,noreferrer');
  }

  /** Cross-origin iframes don't let the parent pause their AudioContext. The
   *  only way to actually silence an iframe that's already playing is to
   *  recycle it. The sessionId is preserved so balance/history survive. */
  function remountIframe(frame: FrameState) {
    if (!frame.src) return;
    const src = frame.src;
    frame.src = null;
    setTimeout(() => {
      frame.src = src;
    }, 30);
  }

  function toggleMute(frame: FrameState) {
    const wasMuted = frame.muted;
    frame.muted = !frame.muted;
    if (!wasMuted && frame.muted) remountIframe(frame);
  }

  function muteAll() {
    for (const f of frames) {
      const wasMuted = f.muted;
      f.muted = true;
      if (!wasMuted) remountIframe(f);
    }
  }
  function unmuteAll() {
    frames.forEach((f) => (f.muted = false));
  }
</script>

<svelte:head>
  <title>Stake Dev Tool · Test view</title>
</svelte:head>

<Toaster position="top-right" richColors closeButton />

<div class="flex h-screen overflow-hidden bg-background text-foreground">
  <!-- Sidebar -->
  <aside
    class="relative flex h-screen flex-shrink-0 flex-col border-r bg-card/30 transition-[width] duration-200 ease-out {sidebarCollapsed
      ? 'w-16'
      : 'w-[440px]'}"
  >
    <!-- Header -->
    <div
      class="flex min-h-[73px] flex-shrink-0 items-center gap-3 border-b {sidebarCollapsed
        ? 'justify-center px-2 py-3'
        : 'justify-between px-5 py-4'}"
    >
      {#if !sidebarCollapsed}
        <div class="min-w-0">
          <p class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Game</p>
          <p class="mt-0.5 truncate text-sm font-semibold">{gameSlug || '—'}</p>
        </div>
      {/if}
      <Tooltip.Provider delayDuration={300}>
        <Tooltip.Root>
          <Tooltip.Trigger>
            {#snippet child({ props })}
              <Button
                {...props}
                variant="outline"
                size="icon"
                class="h-9 w-9 flex-shrink-0 rounded-full border-border/80 bg-background/80 shadow-sm hover:bg-muted"
                onclick={() => (sidebarCollapsed = !sidebarCollapsed)}
                aria-label={sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}
                aria-expanded={!sidebarCollapsed}
              >
                {#if sidebarCollapsed}
                  <ChevronRightIcon class="h-4 w-4" />
                {:else}
                  <ChevronLeftIcon class="h-4 w-4" />
                {/if}
              </Button>
            {/snippet}
          </Tooltip.Trigger>
          <Tooltip.Content>{sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}</Tooltip.Content>
        </Tooltip.Root>
      </Tooltip.Provider>
    </div>

    {#if sidebarCollapsed}
      <div class="flex flex-1 flex-col items-center gap-3 px-2 py-4">
        <div
          class="flex h-9 w-9 items-center justify-center rounded-md border bg-background text-xs font-semibold uppercase text-muted-foreground"
          title={gameSlug || 'Game'}
        >
          G
        </div>
      </div>
    {:else}
      <!-- Scrollable sections -->
      <div class="flex-1 space-y-5 overflow-y-auto px-5 py-4">
      <!-- ========== SESSION ========== -->
      <Card.Root>
        <Card.Header class="pb-3">
          <Card.Title class="text-sm font-semibold uppercase tracking-wider text-muted-foreground">
            Session
          </Card.Title>
        </Card.Header>
        <Card.Content class="space-y-3">
          <div class="space-y-1.5">
            <Label for="initial-balance" class="text-xs uppercase tracking-wider text-muted-foreground">
              Initial balance
            </Label>
            <div class="flex gap-1.5">
              <Input
                id="initial-balance"
                type="number"
                bind:value={balance}
                min={0}
                step={100}
                class="font-mono-tab flex-1"
              />
              <div class="w-36 flex-shrink-0">
                <Picker
                  items={availableCurrencies}
                  value={currency}
                  onSelect={(c) => (currency = c)}
                />
              </div>
            </div>
          </div>

          <div class="grid grid-cols-2 gap-2">
            <div class="space-y-1.5">
              <Label class="text-xs uppercase tracking-wider text-muted-foreground">Lang</Label>
              <Picker items={LANGUAGES} value={language} onSelect={(l) => (language = l)} />
            </div>
            <div class="space-y-1.5">
              <Label for="device-select" class="text-xs uppercase tracking-wider text-muted-foreground">
                Device
              </Label>
              <select
                id="device-select"
                bind:value={device}
                class="border-input bg-background ring-offset-background focus-visible:ring-ring flex h-8 w-full rounded-md border px-2 py-1 font-mono text-xs focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none"
              >
                <option value="desktop">desktop</option>
                <option value="mobile">mobile</option>
              </select>
            </div>
          </div>

          <div class="flex items-center justify-between rounded-md border bg-card/50 px-3 py-2">
            <Label for="social-toggle" class="flex items-center gap-2 text-xs">
              <span>{social ? '🎰' : '💵'}</span>
              <span>Social casino</span>
            </Label>
            <Switch id="social-toggle" checked={social} onCheckedChange={toggleSocial} />
          </div>

          <Button onclick={reloadAll} disabled={busy} class="w-full" size="default">
            <RefreshIcon class="h-4 w-4" />
            Apply &amp; reload all
          </Button>

          <Button
            onclick={resetSessions}
            disabled={busy}
            class="w-full"
            size="default"
            variant="destructive"
          >
            <TrashIcon class="h-4 w-4" />
            Reset sessions
          </Button>

          <div class="grid grid-cols-2 gap-1.5">
            <Button variant="outline" size="sm" onclick={muteAll}>
              <VolumeOffIcon class="h-4 w-4" />
              Mute all
            </Button>
            <Button variant="outline" size="sm" onclick={unmuteAll}>
              <VolumeIcon class="h-4 w-4" />
              Unmute all
            </Button>
          </div>
        </Card.Content>
      </Card.Root>

      <!-- ========== EVENTS ========== -->
      <Card.Root>
        <Card.Header class="pb-3">
          <Card.Title class="text-sm font-semibold uppercase tracking-wider text-muted-foreground">
            Events
          </Card.Title>
        </Card.Header>
        <Card.Content class="space-y-2.5">
          <!-- Force event -->
          <div class="space-y-2 rounded-md border bg-card/50 p-2.5">
            <div class="flex items-center justify-between">
              <span class="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                Force next event
              </span>
              {#if forcedEventBanner}
                <Button
                  variant="ghost"
                  size="sm"
                  class="h-5 px-1.5 text-xs text-amber-400 hover:text-amber-300"
                  onclick={clearForcedEvent}
                  disabled={busy}
                >
                  Clear
                </Button>
              {/if}
            </div>
            <div class="flex gap-1.5">
              <select
                bind:value={forcedMode}
                class="border-input bg-background flex h-8 rounded-md border px-2 py-1 font-mono text-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                {#each availableModes as m (m)}
                  <option value={m}>{m}</option>
                {/each}
              </select>
              <Input
                type="number"
                bind:value={forcedEventId}
                min={1}
                placeholder="eventId"
                class="font-mono-tab h-9 flex-1 text-sm"
              />
              <Button size="sm" class="h-8" onclick={applyForcedEvent} disabled={busy}>
                <ZapIcon class="h-4 w-4" />
                Force
              </Button>
              <Tooltip.Provider delayDuration={300}>
                <Tooltip.Root>
                  <Tooltip.Trigger>
                    {#snippet child({ props })}
                      <Button
                        {...props}
                        variant="outline"
                        size="icon"
                        class="h-9 w-9"
                        disabled={forcedEventId === null || forcedEventId <= 0}
                        onclick={() => (showSaveInput = !showSaveInput)}
                      >
                        <StarIcon class="h-4 w-4" />
                      </Button>
                    {/snippet}
                  </Tooltip.Trigger>
                  <Tooltip.Content>Save this round for later</Tooltip.Content>
                </Tooltip.Root>
              </Tooltip.Provider>
            </div>
            {#if showSaveInput}
              <div class="flex gap-1.5">
                <Input
                  type="text"
                  bind:value={saveDescription}
                  placeholder="Description (optional)"
                  maxlength={120}
                  onkeydown={(e) => e.key === 'Enter' && saveCurrentRound()}
                  class="h-9 flex-1 text-sm"
                />
                <Button size="sm" class="h-8" onclick={saveCurrentRound} disabled={savingRound}>
                  Save
                </Button>
              </div>
            {/if}
            {#if forcedEventBanner}
              <div class="flex items-center gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-2 py-1.5 text-sm">
                <Badge variant="secondary" class="bg-amber-500/20 text-amber-300">Active</Badge>
                <span class="font-mono-tab text-amber-200">
                  {forcedEventBanner.mode} #{forcedEventBanner.eventId}
                </span>
              </div>
            {/if}
          </div>

          <!-- Saved rounds -->
          <div class="rounded-md border bg-card/50">
            <button
              onclick={() => (showSavedRounds = !showSavedRounds)}
              class="flex w-full items-center justify-between gap-2 px-2.5 py-1.5 text-xs font-medium uppercase tracking-wider text-muted-foreground transition hover:text-foreground"
            >
              <span class="flex items-center gap-2">
                <StarIcon class="h-4 w-4" />
                Saved rounds ({savedRounds.length})
              </span>
              <ChevronDownIcon class="h-4 w-4 transition {showSavedRounds ? 'rotate-180' : ''}" />
            </button>
            {#if showSavedRounds}
              <Separator />
              <div class="p-2">
                {#if savedRounds.length === 0}
                  <p class="text-xs text-muted-foreground">
                    No saved rounds yet. Force an event then click ★ to bookmark.
                  </p>
                {:else}
                  <div class="max-h-56 space-y-1 overflow-y-auto">
                    {#each savedRounds as r (r.id)}
                      <div class="group flex items-center gap-1.5 rounded px-1.5 py-1 transition hover:bg-muted/50">
                        <button
                          onclick={() => applySavedRound(r)}
                          disabled={busy}
                          title="Force this round"
                          class="flex min-w-0 flex-1 flex-col items-start text-left"
                        >
                          <span class="flex w-full items-baseline gap-1.5">
                            <span class="font-mono-tab text-sm text-sky-400">{r.mode}</span>
                            <span class="font-mono-tab text-sm font-semibold">#{r.eventId}</span>
                          </span>
                          {#if r.description}
                            <span class="w-full truncate text-xs text-muted-foreground">{r.description}</span>
                          {/if}
                        </button>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7 text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:text-destructive"
                          onclick={() => deleteSavedRound(r)}
                          aria-label="Delete saved round"
                        >
                          <XIcon class="h-4 w-4" />
                        </Button>
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}
          </div>

          <!-- Notable rounds -->
          <div class="rounded-md border bg-card/50">
            <button
              onclick={toggleNotablePanel}
              class="flex w-full items-center justify-between gap-2 px-2.5 py-1.5 text-xs font-medium uppercase tracking-wider text-muted-foreground transition hover:text-foreground"
            >
              <span class="flex items-center gap-2">
                <ZapIcon class="h-4 w-4" />
                Notable rounds
                {#if notableLoaded}
                  <Badge variant="secondary" class="h-4 px-1 text-xs">{notableRounds.length}</Badge>
                {/if}
              </span>
              <ChevronDownIcon class="h-4 w-4 transition {showNotable ? 'rotate-180' : ''}" />
            </button>
            {#if showNotable}
              <Separator />
              <div class="p-2">
                {#if notableLoading}
                  <p class="text-xs text-muted-foreground">Loading…</p>
                {:else if notableRounds.length === 0}
                  <p class="text-xs text-muted-foreground">
                    No modes detected. Make sure the math folder has weights.
                  </p>
                {:else}
                  <p class="mb-1.5 text-xs uppercase tracking-wider text-muted-foreground">
                    Auto-picked from each mode's lookup table.
                  </p>
                  <div class="max-h-72 space-y-2 overflow-y-auto">
                    {#each notableRounds as m (m.mode)}
                      <div class="rounded border bg-background/50 p-2">
                        <div class="mb-1 font-mono-tab text-sm font-semibold text-sky-400">
                          {m.mode}
                        </div>
                        <div class="space-y-0.5">
                          {#each notableBuckets as bucket (bucket.kind)}
                            {#each m.stats[bucket.kind] as bet, ix (`${bucket.kind}-${bet.eventId}-${ix}`)}
                            {@const bk = isBookmarked(m.mode, bet.eventId)}
                            <div class="flex items-center gap-2 rounded px-1.5 py-1 hover:bg-muted/40">
                              <span class="w-9 text-xs uppercase tracking-wider text-muted-foreground">
                                {ix === 0 ? bucket.label : ''}
                              </span>
                              <span class="font-mono-tab text-sm {bucket.color}">
                                #{bet.eventId}
                              </span>
                              <span class="ml-auto font-mono-tab text-xs text-muted-foreground">
                                ×{(bet.payoutMultiplier / 100).toFixed(2)}
                              </span>
                              <Button
                                variant="outline"
                                size="sm"
                                class="h-5 px-1.5 text-xs"
                                onclick={() => applyForcedFromNotable(m.mode, bet.eventId)}
                                disabled={busy}
                              >
                                Force
                              </Button>
                              <Button
                                variant="ghost"
                                size="icon"
                                class="h-7 w-7 {bk
                                  ? 'cursor-default text-amber-400'
                                  : 'text-muted-foreground hover:text-amber-400'}"
                                disabled={bk}
                                onclick={() => bookmarkNotable(m.mode, bet.eventId, bucket.kind)}
                                aria-label={bk ? 'Already bookmarked' : 'Bookmark'}
                              >
                                <StarIcon class="h-4 w-4 {bk ? 'fill-amber-400' : ''}" />
                              </Button>
                            </div>
                            {/each}
                          {/each}
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}
          </div>

          <!-- Replay -->
          <div class="rounded-md border bg-card/50">
            <button
              onclick={() => (showReplay = !showReplay)}
              class="flex w-full items-center justify-between gap-2 px-2.5 py-1.5 text-xs font-medium uppercase tracking-wider text-muted-foreground transition hover:text-foreground"
            >
              <span class="flex items-center gap-2">
                <RewindIcon class="h-4 w-4" />
                Replay event
              </span>
              <ChevronDownIcon class="h-4 w-4 transition {showReplay ? 'rotate-180' : ''}" />
            </button>
            {#if showReplay}
              <Separator />
              <div class="space-y-2 p-2">
                <div class="grid grid-cols-[112px_1fr] gap-1.5">
                  <div class="space-y-1">
                    <Label class="text-xs uppercase tracking-wider text-muted-foreground">
                      Mode
                    </Label>
                    <select
                      bind:value={replayMode}
                      class="border-input bg-background flex h-9 w-full rounded-md border px-2 py-1 font-mono text-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      {#each availableModes as m (m)}
                        <option value={m}>{m}</option>
                      {/each}
                    </select>
                  </div>
                  <div class="space-y-1">
                    <Label class="text-xs uppercase tracking-wider text-muted-foreground">
                      Event ID
                    </Label>
                    <Input
                      type="number"
                      bind:value={replayEventId}
                      min={1}
                      placeholder="eventId"
                      class="font-mono-tab h-9 text-sm"
                    />
                  </div>
                </div>
                <div class="flex gap-1.5">
                  <div class="min-w-0 flex-1 space-y-1">
                    <Label class="text-xs uppercase tracking-wider text-muted-foreground">
                      Bet amount
                    </Label>
                    <Input
                      type="number"
                      bind:value={replayAmount}
                      min={0.01}
                      max={1000}
                      step={0.01}
                      placeholder="0.01 - 1000"
                      class="font-mono-tab h-9 text-sm"
                    />
                  </div>
                  <Button
                    size="sm"
                    class="mt-6 h-9 bg-sky-500 text-zinc-950 hover:bg-sky-400"
                    disabled={busy || frames.length === 0 || replayAmount < 0.01 || replayAmount > 1000}
                    onclick={() => frames[0] && launchReplay(frames[0])}
                  >
                    Load
                  </Button>
                </div>
                <p class="text-xs text-muted-foreground">
                  Loads into the top-left frame. No session, no RNG — just the event outcome.
                </p>
              </div>
            {/if}
          </div>
        </Card.Content>
      </Card.Root>

      <!-- ========== LAYOUT ========== -->
      <Card.Root>
        <Card.Header class="pb-3">
          <Card.Title class="text-sm font-semibold uppercase tracking-wider text-muted-foreground">
            Layout
          </Card.Title>
        </Card.Header>
        <Card.Content>
          <div class="rounded-md border bg-card/50">
            <button
              onclick={() => (showManage = !showManage)}
              class="flex w-full items-center justify-between gap-2 px-2.5 py-1.5 text-sm font-medium transition hover:text-foreground"
            >
              <span class="flex items-center gap-2">
                <LayoutIcon class="h-4 w-4" />
                Resolutions ({allResolutions.filter((r) => r.enabled).length}/{allResolutions.length})
              </span>
              <ChevronDownIcon class="h-4 w-4 text-muted-foreground transition {showManage ? 'rotate-180' : ''}" />
            </button>
            {#if showManage}
              <Separator />
              <div class="p-2">
                <div class="mb-2 max-h-64 space-y-1 overflow-y-auto">
                  {#each allResolutions as r (r.id)}
                    <div class="group flex items-center gap-2 rounded px-1.5 py-1 transition hover:bg-muted/50">
                      <Checkbox
                        id="res-{r.id}"
                        checked={r.enabled}
                        onCheckedChange={(v) => toggleResolution(r.id, v === true)}
                        disabled={busy}
                      />
                      <Label for="res-{r.id}" class="flex-1 cursor-pointer text-xs font-normal">
                        <span>{r.label}</span>
                        <span class="ml-1.5 font-mono-tab text-xs text-muted-foreground">
                          {r.width}×{r.height}
                        </span>
                        {#if !r.builtin}
                          <Badge variant="secondary" class="ml-1 h-4 bg-amber-500/15 px-1 text-xs text-amber-300">
                            custom
                          </Badge>
                        {/if}
                      </Label>
                      {#if !r.builtin}
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7 text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:text-destructive"
                          onclick={() => deleteCustomResolution(r.id)}
                          disabled={busy}
                          aria-label="Delete custom resolution"
                        >
                          <TrashIcon class="h-4 w-4" />
                        </Button>
                      {/if}
                    </div>
                  {/each}
                </div>

                <Separator class="my-2" />
                <div class="space-y-1.5">
                  <p class="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    Add custom
                  </p>
                  <Input
                    type="text"
                    bind:value={newCustomLabel}
                    placeholder="Label (e.g. iPad)"
                    class="h-8 text-xs"
                  />
                  <div class="grid grid-cols-2 gap-1.5">
                    <Input
                      type="number"
                      bind:value={newCustomWidth}
                      min={1}
                      max={4096}
                      placeholder="Width"
                      class="font-mono-tab h-8 text-xs"
                    />
                    <Input
                      type="number"
                      bind:value={newCustomHeight}
                      min={1}
                      max={4096}
                      placeholder="Height"
                      class="font-mono-tab h-8 text-xs"
                    />
                  </div>
                  <Button
                    size="sm"
                    class="w-full"
                    onclick={addCustomResolution}
                    disabled={busy || !newCustomLabel.trim()}
                  >
                    <PlusIcon class="h-4 w-4" />
                    Add
                  </Button>
                </div>
              </div>
            {/if}
          </div>
        </Card.Content>
      </Card.Root>
      </div>
    {/if}
  </aside>

  <!-- Frames area -->
  <main class="flex-1 overflow-auto p-6">
    <div class="flex flex-wrap gap-6">
      {#each frames as frame (frame.res.id)}
        <div class="flex flex-col">
          <!-- Frame header -->
          <div class="mb-2 flex items-center justify-between gap-3">
            <div class="flex items-center gap-2 text-sm">
              <span class="font-semibold">{frame.res.label}</span>
              <span class="font-mono-tab text-muted-foreground">
                {frame.res.width}×{frame.res.height}
              </span>
            </div>
            <div class="flex items-center gap-1">
              <Tooltip.Provider delayDuration={300}>
                <Tooltip.Root>
                  <Tooltip.Trigger>
                    {#snippet child({ props })}
                      <Button
                        {...props}
                        variant="outline"
                        size="icon"
                        class="h-9 w-9 {frame.muted ? 'text-amber-400' : 'text-emerald-400'}"
                        onclick={() => toggleMute(frame)}
                      >
                        {#if frame.muted}
                          <VolumeOffIcon class="h-4 w-4" />
                        {:else}
                          <VolumeIcon class="h-4 w-4" />
                        {/if}
                      </Button>
                    {/snippet}
                  </Tooltip.Trigger>
                  <Tooltip.Content>
                    {frame.muted ? 'Unmute (allow audio)' : 'Mute (suspend audio)'}
                  </Tooltip.Content>
                </Tooltip.Root>

                <Tooltip.Root>
                  <Tooltip.Trigger>
                    {#snippet child({ props })}
                      <Button
                        {...props}
                        variant="outline"
                        size="icon"
                        class="h-9 w-9"
                        onclick={() => reloadOne(frame)}
                        disabled={busy}
                      >
                        <RefreshIcon class="h-4 w-4" />
                      </Button>
                    {/snippet}
                  </Tooltip.Trigger>
                  <Tooltip.Content>Reload this frame</Tooltip.Content>
                </Tooltip.Root>

                <Tooltip.Root>
                  <Tooltip.Trigger>
                    {#snippet child({ props })}
                      <Button
                        {...props}
                        variant="outline"
                        size="icon"
                        class="h-9 w-9"
                        onclick={() => openInBrowser(frame)}
                        disabled={busy || !frame.src}
                      >
                        <ExternalLinkIcon class="h-4 w-4" />
                      </Button>
                    {/snippet}
                  </Tooltip.Trigger>
                  <Tooltip.Content>Open in new tab</Tooltip.Content>
                </Tooltip.Root>
              </Tooltip.Provider>
            </div>
          </div>

          <!-- Last-event strip -->
          <div
            class="mb-1.5 flex items-center gap-3 rounded-md border bg-card/50 px-3 py-2"
            style="width: {frame.res.width}px;"
          >
            {#if frame.history[0]}
              {@const last = frame.history[0]}
              {@const lm = last.payoutMultiplier / 100}
              {@const hit = lm > 0}
              <div class="flex items-baseline gap-3 text-base">
                <span class="text-xs uppercase tracking-wider text-muted-foreground">Last</span>
                <span class="font-mono-tab font-semibold text-sky-400">#{last.eventId}</span>
                <span class="font-mono-tab text-lg font-bold {hit ? 'text-emerald-400' : 'text-muted-foreground'}">
                  ×{lm.toFixed(2)}
                </span>
                <span class="font-mono-tab text-sm text-muted-foreground">
                  bet {formatAmount(last.betAmount)} · win {formatAmount(last.payout)}
                </span>
              </div>
            {:else}
              <span class="text-xs text-muted-foreground">Waiting for first spin…</span>
            {/if}
          </div>

          <!-- Iframe -->
          <div
            class="relative overflow-hidden rounded-lg border bg-black shadow-xl"
            style="width: {frame.res.width}px; height: {frame.res.height}px;"
          >
            {#if frame.src}
              <iframe
                src={frame.src}
                title={frame.res.label}
                class="h-full w-full bg-black"
                allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; fullscreen; cross-origin-isolated"
                allowfullscreen
              ></iframe>
            {:else}
              <div class="flex h-full w-full items-center justify-center text-xs text-muted-foreground">
                Loading…
              </div>
            {/if}

            {#if frame.muted && frame.src}
              <button
                onclick={() => toggleMute(frame)}
                class="group absolute inset-0 z-10 flex cursor-pointer items-center justify-center bg-black/0 transition hover:bg-black/30"
                title="Click to unmute (enable audio + interactions)"
                aria-label="Unmute"
              >
                <span class="rounded-full bg-zinc-900/80 p-2 text-amber-400 opacity-0 ring-1 ring-amber-500/30 transition group-hover:opacity-100">
                  <VolumeOffIcon class="h-4 w-4" />
                </span>
              </button>
            {/if}
          </div>

          <!-- History toggle -->
          <button
            onclick={() => (frame.showHistory = !frame.showHistory)}
            disabled={frame.history.length === 0}
            title="Toggle event history"
            style="width: {frame.res.width}px;"
            class="mt-1.5 flex items-center justify-between gap-2 rounded-md border bg-card/50 px-3 py-2 text-sm font-medium uppercase tracking-wider text-muted-foreground transition hover:bg-muted/50 hover:text-foreground disabled:opacity-40"
          >
            <span class="flex items-center gap-1.5">
              <HistoryIcon class="h-4 w-4" />
              Bet history ({frame.history.length})
            </span>
            <ChevronDownIcon class="h-4 w-4 transition {frame.showHistory ? 'rotate-180' : ''}" />
          </button>

          {#if frame.showHistory && frame.history.length > 0}
            <div
              class="mt-1 overflow-hidden rounded-md border bg-card/50"
              style="width: {frame.res.width}px;"
            >
              <div class="grid grid-cols-[auto_auto_auto_1fr_auto_auto_auto] items-center gap-x-3 border-b bg-muted/30 px-3 py-1.5 text-xs font-medium uppercase tracking-wider text-muted-foreground">
                <span></span>
                <span>#</span>
                <span>Event</span>
                <span>Mode</span>
                <span>Bet</span>
                <span>Mult</span>
                <span>Win</span>
              </div>
              <div class="max-h-72 overflow-y-auto font-mono-tab text-sm">
                {#each frame.history as entry, i (entry.at + '-' + entry.eventId)}
                  {@const hit = entry.payout > 0}
                  {@const bookmarked = isBookmarked(entry.mode, entry.eventId)}
                  <div
                    class="grid grid-cols-[auto_auto_auto_1fr_auto_auto_auto] items-center gap-x-3 border-b px-3 py-1.5 transition hover:bg-muted/30 {entry.forced
                      ? 'bg-amber-500/5'
                      : ''}"
                  >
                    <button
                      onclick={() => openBookmarkModal(entry)}
                      disabled={bookmarked}
                      title={bookmarked ? 'Already bookmarked' : 'Bookmark this round'}
                      class="leading-none transition {bookmarked
                        ? 'cursor-default text-amber-400'
                        : 'text-muted-foreground hover:text-amber-400'}"
                    >
                      {#if bookmarked}
                        <StarIcon class="h-4 w-4 fill-amber-400" />
                      {:else}
                        <StarIcon class="h-4 w-4" />
                      {/if}
                    </button>
                    <span class="text-muted-foreground">{i + 1}</span>
                    <span class="font-semibold text-sky-400">#{entry.eventId}</span>
                    <span class="truncate text-muted-foreground">
                      {entry.mode}
                      {#if entry.forced}
                        <Badge variant="secondary" class="ml-1 h-4 bg-amber-500/20 px-1 text-xs text-amber-300">
                          FORCED
                        </Badge>
                      {/if}
                    </span>
                    <span class="text-muted-foreground">{formatAmount(entry.betAmount)}</span>
                    <span class={hit ? 'text-emerald-400' : 'text-muted-foreground'}>
                      ×{(entry.payoutMultiplier / 100).toFixed(2)}
                    </span>
                    <span class={hit ? 'text-emerald-400' : 'text-muted-foreground'}>
                      {formatAmount(entry.payout)}
                    </span>
                  </div>
                {/each}
              </div>
              <div class="border-t bg-muted/20 px-3 py-1 text-xs text-muted-foreground">
                Last 100 spins, newest first. Persists until sessions are reset.
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  </main>
</div>

<!-- Bookmark dialog -->
<Dialog.Root open={bookmarkModal !== null} onOpenChange={(o) => !o && closeBookmarkModal()}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>Bookmark this round</Dialog.Title>
      <Dialog.Description>
        Save this event id for quick replay later.
      </Dialog.Description>
    </Dialog.Header>
    {#if bookmarkModal}
      <div class="flex items-baseline gap-2 rounded-md border bg-card/50 px-3 py-2">
        <span class="text-xs uppercase tracking-wider text-muted-foreground">Round</span>
        <span class="font-mono-tab text-sm text-sky-400">{bookmarkModal.mode}</span>
        <span class="font-mono-tab text-sm font-semibold">#{bookmarkModal.eventId}</span>
      </div>
      <div class="space-y-1.5">
        <Label for="bookmark-description" class="text-xs uppercase tracking-wider text-muted-foreground">
          Description (optional)
        </Label>
        <Input
          id="bookmark-description"
          type="text"
          bind:ref={bookmarkInputEl}
          bind:value={bookmarkModal.description}
          placeholder="e.g. Big bonus trigger, near miss, …"
          maxlength={120}
          onkeydown={(e) => {
            if (e.key === 'Enter') confirmBookmark();
          }}
        />
      </div>
      <Dialog.Footer>
        <Button variant="outline" onclick={closeBookmarkModal}>Cancel</Button>
        <Button onclick={confirmBookmark} disabled={bookmarkModal.saving}>
          <StarIcon class="h-4 w-4" />
          {bookmarkModal.saving ? 'Saving…' : 'Bookmark'}
        </Button>
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
