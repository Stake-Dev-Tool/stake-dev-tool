<script lang="ts">
  import { onMount } from 'svelte';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Sheet from '$lib/components/ui/sheet';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { Switch } from '$lib/components/ui/switch';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import { Separator } from '$lib/components/ui/separator';
  import { Badge } from '$lib/components/ui/badge';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { Toaster } from '$lib/components/ui/sonner';
  import { toast } from 'svelte-sonner';

  import PlayIcon from '@lucide/svelte/icons/play';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
  import ShieldIcon from '@lucide/svelte/icons/shield-check';
  import ShieldAlertIcon from '@lucide/svelte/icons/shield-alert';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import MonitorIcon from '@lucide/svelte/icons/monitor';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import MinimizeIcon from '@lucide/svelte/icons/minimize-2';
  import UsersIcon from '@lucide/svelte/icons/users';
  import UploadIcon from '@lucide/svelte/icons/upload';
  import { goto } from '$app/navigation';

  import {
    browser,
    ca,
    checkForUpdates,
    cloudApi,
    downloadAndInstallUpdate,
    githubAuth,
    lgs,
    onWorkspaceEvent,
    pickFolder,
    profiles as profilesApi,
    settings as settingsApi,
    teamsApi,
    type CaStatus,
    type CloudUser,
    type GithubUser,
    type InspectedGame,
    type LgsStatus,
    type Profile,
    type ResolutionPreset,
    type Team,
    type TeamProfileInfo,
    type UpdateInfo
  } from '$lib/api';
  import * as Dialog from '$lib/components/ui/dialog';
  import GithubSignInDialog from '$lib/components/GithubSignInDialog.svelte';
  import CloudSignInDialog from '$lib/components/CloudSignInDialog.svelte';
  import KeyRoundIcon from '@lucide/svelte/icons/key-round';
  import LogOutIcon from '@lucide/svelte/icons/log-out';
  import LogInIcon from '@lucide/svelte/icons/log-in';
  import SettingsIcon from '@lucide/svelte/icons/settings-2';

  import DownloadCloudIcon from '@lucide/svelte/icons/download-cloud';
  import CloudIcon from '@lucide/svelte/icons/cloud';
  import UsersRoundIcon from '@lucide/svelte/icons/users-round';
  import Share2Icon from '@lucide/svelte/icons/share-2';
  import GlobeIcon from '@lucide/svelte/icons/globe';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';

  const LGS_PORT_LS = 'sdt.lgsPort';
  const DEFAULT_PORT = 3001;
  const LS_CLOSE_AFTER = 'sdt.closeAfterLaunch';

  // Persisted port choice (1024-65535). Stored as number; falls back to default.
  let lgsPort = $state<number>(DEFAULT_PORT);
  let portSettingsOpen = $state(false);

  // GitHub auth state — surfaced in the topbar so sign-in is reachable from
  // anywhere (not buried behind the Teams page).
  // GitHub auth — kept for the Share-preview feature (GitHub Pages hosting).
  let githubUser = $state<GithubUser | null>(null);
  let signInOpen = $state(false);

  // Cloud auth + active workspace — drives the workspace catalogue (share/pull)
  // and the live SSE stream; surfaced in the topbar.
  let cloudUser = $state<CloudUser | null>(null);
  let cloudSignInOpen = $state(false);
  let activeWorkspace = $state<Team | null>(null);
  // The workspace we currently hold an SSE subscription for. Guards against
  // re-subscribing on every refresh (a resubscribe would re-emit `reconnected`,
  // which would trigger another refresh → subscribe loop).
  let subscribedWorkspaceId: string | null = null;

  function ensureSubscribed(id: string | null) {
    if (!id || id === subscribedWorkspaceId) return;
    subscribedWorkspaceId = id;
    cloudApi.subscribe(id).catch(() => {});
  }

  async function refreshGithubUser() {
    try {
      githubUser = await githubAuth.currentUser();
    } catch {
      githubUser = null;
    }
  }

  async function onGithubSignedIn(u: GithubUser) {
    githubUser = u;
  }

  async function signOutGithub() {
    if (!confirm('Sign out of GitHub? Share-preview publishing will be disabled.')) return;
    try {
      await githubAuth.logout();
      githubUser = null;
      toast.success('Signed out of GitHub');
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }

  async function refreshCloudUser() {
    try {
      cloudUser = await cloudApi.currentUser();
    } catch {
      cloudUser = null;
    }
  }

  async function onCloudSignedIn(u: CloudUser) {
    cloudUser = u;
    await refreshTeamsAndCatalog();
  }

  async function signOutCloud() {
    if (!confirm('Sign out of the cloud? Workspace syncing will pause here.')) return;
    try {
      await cloudApi.unsubscribe().catch(() => {});
      await cloudApi.signOut();
      cloudUser = null;
      activeWorkspace = null;
      allTeams = [];
      catalog = [];
      toast.success('Signed out of the cloud');
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }

  function clampPort(p: number): number {
    if (!Number.isFinite(p)) return DEFAULT_PORT;
    return Math.min(65535, Math.max(1024, Math.floor(p)));
  }

  function applyLgsPort(next: number) {
    lgsPort = clampPort(next);
    localStorage.setItem(LGS_PORT_LS, String(lgsPort));
  }

  let status = $state<LgsStatus>({ running: false, bound_addr: null, math_dir: null });
  // `null` until the first ca.status() round-trip completes — prevents the
  // "Install Local CA" banner from flashing for ~300ms when returning to this
  // page (e.g. from /teams), which happens because `installed: false` is the
  // default but the real state usually IS installed.
  let caState = $state<CaStatus | null>(null);
  let updateInfo = $state<UpdateInfo | null>(null);
  let checkingUpdate = $state(false);
  let installingUpdate = $state(false);
  let updateProgress = $state<{ downloaded: number; total?: number } | null>(null);

  let savedProfiles = $state<Profile[]>([]);
  let activeProfileId = $state<string | null>(null);

  type Draft = {
    mode: 'new' | 'edit';
    id?: string;
    name: string;
    game: InspectedGame | null;
    gameUrl: string;
  };
  let draft = $state<Draft | null>(null);
  let drawerOpen = $state(false);

  let resolutions = $state<ResolutionPreset[]>([]);
  let showResolutions = $state(false);

  let closeAfterLaunch = $state(true);
  let busy = $state(false);

  let allTeams = $state<Team[]>([]);
  let catalog = $state<import('$lib/api').CatalogEntry[]>([]);
  let catalogLoading = $state(false);
  let readyProfiles = $state<Record<string, boolean>>({});

  // Push-target picker state. Shown when a local profile has no team origin
  // AND the user is in more than one team — user then explicitly picks where
  // the profile lands instead of us silently defaulting.
  let pushPickerOpen = $state(false);
  let pushPickerProfile = $state<Profile | null>(null);

  // Share preview state.
  let shareOpen = $state(false);
  let shareProfile = $state<Profile | null>(null);
  let shareFrontPath = $state('');
  let shareMathMode = $state<import('$lib/api').MathMode>('partial');
  let shareBusy = $state(false);
  let shareUrl = $state<string | null>(null);

  const FRONT_PATH_LS_PREFIX = 'sdt.frontPath.';
  const PREVIEW_URL_LS_PREFIX = 'sdt.previewUrl.';
  const MATH_MODE_LS_KEY = 'sdt.previewMathMode';

  onMount(() => {
    (async () => {
      try {
        const stored = localStorage.getItem(LS_CLOSE_AFTER);
        if (stored !== null) closeAfterLaunch = stored === '1';

        const storedPort = localStorage.getItem(LGS_PORT_LS);
        if (storedPort !== null) {
          const parsed = Number.parseInt(storedPort, 10);
          if (Number.isFinite(parsed)) lgsPort = clampPort(parsed);
        }

        status = await lgs.status();
        caState = await ca.status();
        await loadProfiles(true);
        const s = await settingsApi.get();
        resolutions = s.resolutions;
        refreshProfileReadiness().catch(() => {});
        refreshCloudUser().catch(() => {});
        refreshTeamsAndCatalog().catch(() => {});
        refreshGithubUser().catch(() => {});
      } catch (e) {
        console.error(e);
      }
      checkUpdate(true).catch(() => {});
    })();

    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return;
      if (drawerOpen) return;
      if (e.key === 'Enter' && savedProfiles.length > 0) {
        const active = savedProfiles.find((p) => p.id === activeProfileId) ?? savedProfiles[0];
        launchProfile(active);
      } else if (e.key === 'n' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        openNewDraft();
      }
    };
    // Auto-refresh catalogue + readiness when the window regains focus. Keeps
    // the "update available" badges accurate without the user clicking
    // refresh — cheap (a few JSON fetches) and invisible.
    const onFocus = () => {
      if (busy) return;
      refreshTeamsAndCatalog().catch(() => {});
      refreshProfileReadiness().catch(() => {});
    };
    // Live workspace stream: on any document/revision change (or reconnect),
    // refresh the catalogue + readiness so shared saved rounds/profiles appear
    // without a manual refresh (the M3 "two desktops see each other" path).
    let unlistenWs: (() => void) | null = null;
    onWorkspaceEvent((ev) => {
      if (busy) return;
      // The launcher shows the profile catalogue + math readiness. Only refresh
      // for events that affect those (a shared/updated profile, a new math
      // revision, or a reconnect). Saved-round changes don't show here, and
      // reacting to every one would be needless churn.
      const relevant =
        ev.type === 'reconnected' ||
        ev.type === 'revision_pushed' ||
        (ev.type === 'document' && ev.docKind === 'profile');
      if (!relevant) return;
      refreshTeamsAndCatalog().catch(() => {});
      refreshProfileReadiness().catch(() => {});
    })
      .then((un) => {
        unlistenWs = un;
      })
      .catch(() => {});

    window.addEventListener('keydown', onKey);
    window.addEventListener('focus', onFocus);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('focus', onFocus);
      unlistenWs?.();
    };
  });

  function persistCloseAfter() {
    localStorage.setItem(LS_CLOSE_AFTER, closeAfterLaunch ? '1' : '0');
  }

  async function withBusy<T>(fn: () => Promise<T>): Promise<T | undefined> {
    busy = true;
    try {
      return await fn();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  function abbreviatePath(p: string, max = 52): string {
    if (p.length <= max) return p;
    const head = p.slice(0, 12);
    const tail = p.slice(p.length - (max - 14));
    return `${head}…${tail}`;
  }

  const TECHNICAL_FOLDER_SLUGS = new Set(['publish_files', 'publish-files', 'build', 'dist']);

  function slugifyGameName(value: string): string {
    return value
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '');
  }

  function profileGameSlug(p: Pick<Profile, 'name' | 'gameSlug'>, inspectedSlug?: string): string {
    const nameSlug = slugifyGameName(p.name);
    if (!p.gameSlug) return nameSlug || inspectedSlug || '';
    if (p.gameSlug === inspectedSlug && nameSlug) return nameSlug;
    if (TECHNICAL_FOLDER_SLUGS.has(p.gameSlug) && nameSlug) return nameSlug;
    return p.gameSlug;
  }

  async function loadProfiles(repair = false) {
    let profiles = await profilesApi.list();
    if (repair) {
      let changed = false;
      for (const p of profiles) {
        const repairedSlug = profileGameSlug(p);
        if (repairedSlug && repairedSlug !== p.gameSlug) {
          await profilesApi.save({
            id: p.id,
            name: p.name,
            gamePath: p.gamePath,
            gameUrl: p.gameUrl,
            gameSlug: repairedSlug,
            resolutions: p.resolutions
          });
          changed = true;
        }
      }
      if (changed) profiles = await profilesApi.list();
    }
    savedProfiles = profiles;
  }

  // Profiles created by the Cloud browser's "Pull to profile" are named
  // "<game> rev N (cloud)". Read that suffix to show a "cloud rev N" badge on
  // the launcher (the marker convention — see cloud::sync::pull_revision_to_profile).
  function cloudRevOf(name: string): number | null {
    const m = /\brev\s+(\d+)\s+\(cloud\)\s*$/i.exec(name);
    return m ? Number.parseInt(m[1], 10) : null;
  }

  function formatRelative(ts: number | undefined): string {
    if (!ts) return '—';
    const diff = Date.now() - ts;
    if (diff < 60_000) return 'just now';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    const d = Math.floor(diff / 86_400_000);
    return d === 1 ? 'yesterday' : `${d}d ago`;
  }

  async function ensureLgsRunning(mathDir: string) {
    if (status.running && status.math_dir === mathDir) return;
    if (status.running) status = await lgs.stop();
    status = await lgs.start(lgsPort, mathDir);
  }

  async function applyLgsPortAndRestart(next: number) {
    const port = clampPort(next);
    if (port === lgsPort && !status.running) {
      applyLgsPort(port);
      portSettingsOpen = false;
      return;
    }
    applyLgsPort(port);
    if (status.running && status.math_dir) {
      const mathDir = status.math_dir;
      try {
        status = await lgs.stop();
        status = await lgs.start(port, mathDir);
        toast.success(`LGS restarted on port ${port}`);
      } catch (e) {
        toast.error(e instanceof Error ? e.message : String(e));
      }
    } else {
      toast.success(`Port set to ${port}. Will be used next time the LGS starts.`);
    }
    portSettingsOpen = false;
  }

  async function toggleLgs() {
    await withBusy(async () => {
      if (status.running) {
        status = await lgs.stop();
        toast.success('LGS stopped');
      } else {
        const p = savedProfiles.find((x) => x.id === activeProfileId) ?? savedProfiles[0];
        if (!p) throw new Error('Add a profile first.');
        const inspected = await lgs.inspect(p.gamePath);
        const gameSlug = profileGameSlug(p, inspected.slug);
        const mathRoot = gameSlug === inspected.slug ? inspected.mathDir : p.gamePath;
        await ensureLgsRunning(mathRoot);
        toast.success(`LGS listening on ${status.bound_addr}`);
      }
    });
  }

  async function checkUpdate(silent = false) {
    checkingUpdate = true;
    try {
      updateInfo = await checkForUpdates();
      if (!silent) {
        toast.success(
          updateInfo.available
            ? `Update available: v${updateInfo.version}`
            : `Up to date (v${updateInfo.currentVersion})`
        );
      }
    } catch (e) {
      if (!silent) toast.error(`Update check failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      checkingUpdate = false;
    }
  }

  async function installUpdate() {
    if (!updateInfo?.available) return;
    installingUpdate = true;
    updateProgress = { downloaded: 0, total: undefined };
    try {
      await downloadAndInstallUpdate((d, t) => {
        updateProgress = { downloaded: d, total: t };
      });
      toast.success('Update installed — restarting…');
    } catch (e) {
      toast.error(`Update failed: ${e instanceof Error ? e.message : String(e)}`);
      installingUpdate = false;
      updateProgress = null;
    }
  }

  async function installCa() {
    await withBusy(async () => {
      caState = await ca.install();
      if (caState.installed) toast.success('Local CA installed');
      else toast.error('CA install completed but verification failed');
    });
  }

  async function uninstallCa() {
    await withBusy(async () => {
      caState = await ca.uninstall();
      toast.success('Local CA uninstalled');
    });
  }

  async function toggleResolution(id: string, enabled: boolean) {
    await withBusy(async () => {
      const s = await settingsApi.toggle(id, enabled);
      resolutions = s.resolutions;
    });
  }

  async function deleteCustomResolution(id: string) {
    if (!confirm('Delete this custom resolution?')) return;
    await withBusy(async () => {
      const s = await settingsApi.deleteCustom(id);
      resolutions = s.resolutions;
    });
  }

  function openNewDraft() {
    draft = {
      mode: 'new',
      name: '',
      game: null,
      gameUrl: 'http://localhost:5174'
    };
    drawerOpen = true;
  }

  function openEditDraft(p: Profile) {
    draft = {
      mode: 'edit',
      id: p.id,
      name: p.name,
      game: null,
      gameUrl: p.gameUrl
    };
    drawerOpen = true;
    (async () => {
      try {
        const g = await lgs.inspect(p.gamePath);
        if (draft && draft.id === p.id) draft.game = g;
      } catch (e) {
        toast.error(e instanceof Error ? e.message : String(e));
      }
    })();
  }

  function closeDraft() {
    drawerOpen = false;
    setTimeout(() => {
      draft = null;
    }, 250);
  }

  async function pickDraftFolder() {
    if (!draft) return;
    const dir = await pickFolder('Select the math folder (containing index.json)');
    if (!dir) return;
    await withBusy(async () => {
      const g = await lgs.inspect(dir);
      if (!draft) return;
      draft.game = g;
      if (!draft.name.trim()) draft.name = g.slug;
    });
  }

  async function saveDraft() {
    if (!draft) return;
    if (!draft.game) return toast.error('Pick a math folder first');
    if (!draft.name.trim()) return toast.error('Give this profile a name');
    if (!draft.gameUrl.trim()) return toast.error('Enter the front URL');
    const d = draft;
    await withBusy(async () => {
      const saved = await profilesApi.save({
        id: d.mode === 'edit' ? d.id : undefined,
        name: d.name.trim(),
        gamePath: d.game!.gamePath,
        gameUrl: d.gameUrl.trim(),
        gameSlug: slugifyGameName(d.name) || d.game!.slug,
        resolutions
      });
      activeProfileId = saved.id;
      await loadProfiles();
      closeDraft();
      toast.success(`Profile "${saved.name}" ${d.mode === 'edit' ? 'updated' : 'saved'}`);
    });
  }

  async function refreshProfileReadiness() {
    // Update per-profile so a slow `inspect` on one path doesn't keep every
    // other profile's Launch button disabled. Each result lands as soon as
    // its `inspect` resolves.
    await Promise.all(
      savedProfiles.map(async (p) => {
        try {
          await lgs.inspect(p.gamePath);
          readyProfiles = { ...readyProfiles, [p.id]: true };
        } catch {
          readyProfiles = { ...readyProfiles, [p.id]: false };
        }
      })
    );
  }

  async function refreshTeamsAndCatalog() {
    catalogLoading = true;
    try {
      allTeams = await teamsApi.list();
      activeWorkspace = await teamsApi.active();
      // Keep the live stream pointed at the active workspace (subscribe-once).
      ensureSubscribed(activeWorkspace?.id ?? null);
      if (allTeams.length > 0) {
        catalog = await teamsApi.allCatalogs();
        // The backend reconciles local profiles against the catalogue during
        // `allCatalogs` (stamping team_id on profiles whose id appears in a
        // team's catalogue). Re-read profiles so the UI picks up those new
        // team_ids and moves the cards to their real group.
        await loadProfiles();
      } else {
        catalog = [];
      }
    } catch (e) {
      console.error(e);
    } finally {
      catalogLoading = false;
    }
  }

  /// Entry-point for the Push icon. Skips the picker when the target is
  /// unambiguous (profile already linked to a team, or user only has one
  /// team); shows a picker dialog otherwise.
  function onPushClick(p: Profile) {
    if (p.teamId) {
      pushProfileToTeam(p, p.teamId);
      return;
    }
    if (allTeams.length === 0) {
      toast.error('Join or create a team first.');
      return;
    }
    if (allTeams.length === 1) {
      pushProfileToTeam(p, allTeams[0].id);
      return;
    }
    pushPickerProfile = p;
    pushPickerOpen = true;
  }

  async function pushProfileToTeam(p: Profile, teamId: string) {
    const team = allTeams.find((t) => t.id === teamId);
    if (!team) return toast.error('Team not found.');
    await withBusy(async () => {
      toast.info(`Adding "${p.name}" to "${team.name}" catalogue…`);
      await teamsApi.pushProfile(team.id, p.id);
      const gameSlug = profileGameSlug(p);
      toast.info(`Uploading ${gameSlug} math (can take several minutes)…`);
      const r = await teamsApi.pushMath(team.id, gameSlug, p.gamePath);
      const mb = (r.bytesUploaded / 1_048_576).toFixed(1);
      if (r.filesUploaded === 0 && r.filesSkipped > 0) {
        toast.success(`Shared "${p.name}" — math already in sync`);
      } else {
        toast.success(`Shared "${p.name}" with ${mb} MB of math`);
      }
      await loadProfiles();
      refreshTeamsAndCatalog().catch(() => {});
    });
  }

  async function pullTeamProfile(teamId: string, tp: TeamProfileInfo) {
    const team = allTeams.find((t) => t.id === teamId);
    if (!team) return toast.error('Team not found.');
    await withBusy(async () => {
      toast.info(`Pulling "${tp.name}" from "${team.name}"… large games can take several minutes.`);
      const p = await teamsApi.pullProfile(team.id, tp.id);
      await loadProfiles();
      activeProfileId = p.id;
      await refreshTeamsAndCatalog();
      await refreshProfileReadiness();
      toast.success(`Pulled "${p.name}" — ready to launch`);
    });
  }

  async function pullMissingMath(p: Profile) {
    const teamId = p.teamId;
    if (!teamId) {
      return toast.error('This profile has no team origin. Delete it or add the math manually.');
    }
    const tp = catalog.find((c) => c.teamId === teamId && c.profile.id === p.id);
    if (!tp) {
      return toast.error(
        "This game isn't in the team's catalogue anymore. Ask the team owner to push it, or delete this profile."
      );
    }
    await pullTeamProfile(teamId, tp.profile);
  }

  async function removeFromCatalog(p: Profile) {
    if (!p.teamId) return;
    const team = allTeams.find((t) => t.id === p.teamId);
    if (!team) return;
    if (
      !confirm(
        `Remove "${p.name}" from team "${team.name}"?\n\n` +
          `This deletes the profile, math files and saved rounds from the team on GitHub. ` +
          `Other members will lose access. Your local copy is kept.`
      )
    )
      return;
    await withBusy(async () => {
      await teamsApi.removeFromCatalog(team.id, p.id);
      await refreshTeamsAndCatalog();
      toast.success(`Removed "${p.name}" from "${team.name}"`);
    });
  }

  function openShareDialog(p: Profile) {
    shareProfile = p;
    shareFrontPath = localStorage.getItem(FRONT_PATH_LS_PREFIX + p.id) ?? '';
    const stored = localStorage.getItem(MATH_MODE_LS_KEY);
    shareMathMode =
      stored === 'full' || stored === 'partial' || stored === 'sampled'
        ? stored
        : 'sampled';
    // Restore the last published URL so the user can copy it again without
    // re-publishing.
    shareUrl = localStorage.getItem(PREVIEW_URL_LS_PREFIX + p.id);
    shareOpen = true;
  }

  async function pickShareFolder() {
    const dir = await pickFolder('Select the built front folder (contains index.html)');
    if (!dir) return;
    shareFrontPath = dir;
  }

  async function publishShare() {
    if (!shareProfile) return;
    const front = shareFrontPath.trim();
    if (!front) return toast.error('Pick the built front folder first');
    localStorage.setItem(FRONT_PATH_LS_PREFIX + shareProfile.id, front);
    localStorage.setItem(MATH_MODE_LS_KEY, shareMathMode);
    shareBusy = true;
    try {
      toast.info(`Publishing "${shareProfile.name}"… can take 30-90 sec.`);
      const r = await teamsApi.publishPreview(shareProfile.id, front, shareMathMode);
      shareUrl = r.url;
      localStorage.setItem(PREVIEW_URL_LS_PREFIX + shareProfile.id, r.url);
      toast.success(`Published — ${r.filesUploaded} files, ${(r.bytesUploaded / 1_048_576).toFixed(1)} MB`);
    } catch (e) {
      // Keep publish errors on screen until dismissed — they're often the
      // only signal of a config/backend problem and they're long enough that
      // a 4-second auto-dismiss eats them before the user can read.
      const msg = e instanceof Error ? e.message : String(e);
      console.error('[publish] failed:', e);
      toast.error(msg, { duration: Infinity, closeButton: true });
    } finally {
      shareBusy = false;
    }
  }

  async function copyShareUrl() {
    if (!shareUrl) return;
    try {
      await navigator.clipboard.writeText(shareUrl);
      toast.success('URL copied');
    } catch {
      toast.error('Could not copy');
    }
  }

  async function unpublishShare() {
    if (!shareProfile) return;
    if (!confirm(`Take down the preview for "${shareProfile.name}"?`)) return;
    shareBusy = true;
    try {
      await teamsApi.unpublishPreview(shareProfile.id);
      localStorage.removeItem(PREVIEW_URL_LS_PREFIX + shareProfile.id);
      shareUrl = null;
      toast.success('Preview unpublished');
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      shareBusy = false;
    }
  }

  /// Refresh an already-pulled team profile from the remote. Re-downloads only
  /// the chunks whose SHA changed, so it's fast if nothing moved.
  async function refreshFromTeam(p: Profile) {
    const teamId = p.teamId;
    if (!teamId) return;
    const team = allTeams.find((t) => t.id === teamId);
    if (!team) return toast.error('Team not found.');
    await withBusy(async () => {
      toast.info(`Pulling latest "${p.name}" from "${team.name}"…`);
      await teamsApi.pullProfile(team.id, p.id);
      await loadProfiles();
      await refreshProfileReadiness();
      await refreshTeamsAndCatalog();
      toast.success(`"${p.name}" up to date`);
    });
  }

  async function deleteProfile(p: Profile) {
    if (!confirm(`Delete profile "${p.name}"?`)) return;
    await withBusy(async () => {
      await profilesApi.remove(p.id);
      if (activeProfileId === p.id) activeProfileId = null;
      await loadProfiles();
      toast.success(`Deleted "${p.name}"`);
    });
  }

  async function launchProfile(p: Profile) {
    await withBusy(async () => {
      const inspected = await lgs.inspect(p.gamePath);
      const gameSlug = profileGameSlug(p, inspected.slug);
      const mathRoot = gameSlug === inspected.slug ? inspected.mathDir : p.gamePath;
      await ensureLgsRunning(mathRoot);

      if (p.resolutions && p.resolutions.length > 0) {
        const s = await settingsApi.replace(p.resolutions);
        resolutions = s.resolutions;
      }

      const params = new URLSearchParams({
        gameUrl: p.gameUrl,
        gameSlug,
        v: String(Date.now())
      });
      const port = (status.bound_addr ?? '').split(':').pop() ?? `${lgsPort}`;
      const testUrl = `https://localhost:${port}/test/?${params.toString()}`;

      try {
        const r = await browser.openTest(testUrl);
        toast.success(`Opened test view (${r.method})`);
      } catch {
        await openUrl(testUrl);
        toast.success('Opened in default browser');
      }

      activeProfileId = p.id;

      if (closeAfterLaunch) {
        await new Promise((r) => setTimeout(r, 500));
        try {
          await getCurrentWindow().minimize();
        } catch {
          // ignore
        }
      }
    });
  }

  async function saveResolutionsToActiveProfile() {
    const id = activeProfileId ?? savedProfiles[0]?.id;
    if (!id) return toast.error('Add a profile first');
    const p = savedProfiles.find((x) => x.id === id);
    if (!p) return;
    await withBusy(async () => {
      await profilesApi.save({
        id: p.id,
        name: p.name,
        gamePath: p.gamePath,
        gameUrl: p.gameUrl,
        gameSlug: profileGameSlug(p),
        resolutions
      });
      await loadProfiles();
      toast.success(`Snapshot saved to "${p.name}"`);
    });
  }

  const enabledResCount = $derived(resolutions.filter((r) => r.enabled).length);
  const hasProfiles = $derived(savedProfiles.length > 0 || catalog.length > 0);
  const currentVersion = $derived(updateInfo?.currentVersion ?? '');
  const localIdSet = $derived(new Set(savedProfiles.map((p) => p.id)));

  type GameGroup = {
    key: string;
    teamId: string | null;
    teamName: string | null;
    local: Profile[];
    available: { teamId: string; tp: TeamProfileInfo }[];
  };

  const gameGroups: GameGroup[] = $derived.by(() => {
    const mine: Profile[] = [];
    const byTeam = new Map<string, { teamName: string; local: Profile[] }>();

    for (const p of savedProfiles) {
      if (!p.teamId) {
        mine.push(p);
      } else {
        const t = allTeams.find((tt) => tt.id === p.teamId);
        const entry = byTeam.get(p.teamId) ?? { teamName: t?.name ?? '(unknown team)', local: [] };
        entry.local.push(p);
        byTeam.set(p.teamId, entry);
      }
    }

    const availableByTeam = new Map<string, { teamId: string; tp: TeamProfileInfo }[]>();
    for (const c of catalog) {
      if (!c.profile.hasMath) continue;
      if (localIdSet.has(c.profile.id)) continue;
      const list = availableByTeam.get(c.teamId) ?? [];
      list.push({ teamId: c.teamId, tp: c.profile });
      availableByTeam.set(c.teamId, list);
    }

    const groups: GameGroup[] = [];
    if (mine.length > 0) {
      groups.push({
        key: 'mine',
        teamId: null,
        teamName: null,
        local: mine,
        available: []
      });
    }
    // Preserve team order (most recently added first).
    for (const team of allTeams) {
      const b = byTeam.get(team.id);
      const a = availableByTeam.get(team.id) ?? [];
      if (!b && a.length === 0) continue;
      groups.push({
        key: `team-${team.id}`,
        teamId: team.id,
        teamName: team.name,
        local: b?.local ?? [],
        available: a
      });
    }
    return groups;
  });
</script>

<svelte:head>
  <title>Stake Dev Tool</title>
</svelte:head>

<Toaster position="top-right" richColors closeButton />

<main class="mx-auto flex min-h-screen w-full max-w-4xl flex-col gap-8 px-8 py-10">
  <!-- Topbar -->
  <header class="flex items-center justify-between">
    <div class="flex items-center gap-4">
      <div class="flex h-10 w-10 items-center justify-center rounded-xl border bg-card text-lg font-semibold">
        S
      </div>
      <div>
        <h1 class="text-2xl font-semibold tracking-tight">Stake Dev Tool</h1>
        <p class="text-sm text-muted-foreground">
          {#if currentVersion}
            <span class="font-mono-tab">v{currentVersion}</span> ·
          {/if}
          Slot game workbench
        </p>
      </div>
    </div>

    <div class="flex items-center gap-3">
      <Tooltip.Provider delayDuration={200}>
        <!-- Group 1: LGS state + port (segmented pill) — primary affordance.
             Wrapper is h-9 (matching Button size="default") so it aligns with
             the sibling GitHub button without a 2px border-induced offset. -->
        <div class="flex h-9 items-stretch overflow-hidden rounded-md border bg-card shadow-sm">
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <button
                  {...props}
                  type="button"
                  onclick={toggleLgs}
                  disabled={busy || (!status.running && !hasProfiles)}
                  class="flex items-center gap-2 px-3 text-sm transition hover:bg-muted/60 disabled:pointer-events-none disabled:opacity-50"
                >
                  {#if status.running}
                    <span class="status-dot status-dot-live pulse-gentle"></span>
                    <span class="font-mono-tab">{status.bound_addr}</span>
                  {:else}
                    <span class="status-dot status-dot-off"></span>
                    <span>LGS</span>
                    <span class="font-mono-tab text-muted-foreground">:{lgsPort}</span>
                  {/if}
                </button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>{status.running ? 'Click to stop LGS' : 'Click to start LGS'}</Tooltip.Content>
          </Tooltip.Root>

          <div class="w-px self-stretch bg-border"></div>

          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <button
                  {...props}
                  type="button"
                  onclick={() => (portSettingsOpen = true)}
                  aria-label="LGS port settings"
                  class="flex items-center justify-center px-2.5 text-muted-foreground transition hover:bg-muted/60 hover:text-foreground"
                >
                  <SettingsIcon class="h-4 w-4" />
                </button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>LGS port settings</Tooltip.Content>
          </Tooltip.Root>
        </div>

        <!-- Group 2: Account — cloud workspace identity -->
        {#if cloudUser}
          {#if activeWorkspace}
            <Badge variant="outline" class="gap-1 font-normal">
              <UsersRoundIcon class="h-3.5 w-3.5 text-emerald-500" />
              {activeWorkspace.name}
            </Badge>
          {/if}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="outline"
                  size="default"
                  onclick={signOutCloud}
                  class="gap-2"
                >
                  <KeyRoundIcon class="h-4 w-4 text-emerald-500" />
                  <span class="font-mono-tab text-sm">{cloudUser?.display_name ?? ''}</span>
                  <LogOutIcon class="h-3.5 w-3.5 opacity-50" />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Signed in to the cloud — click to sign out</Tooltip.Content>
          </Tooltip.Root>
        {:else}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="outline"
                  size="default"
                  onclick={() => (cloudSignInOpen = true)}
                  class="gap-2"
                >
                  <LogInIcon class="h-4 w-4" />
                  <span class="text-sm">Sign in</span>
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Cloud — required for workspaces</Tooltip.Content>
          </Tooltip.Root>
        {/if}

        <!-- Group 3: Navigation — tertiary -->
        <div class="ml-1 flex items-center gap-1 border-l pl-3">
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon"
                  onclick={() => goto('/cloud')}
                  aria-label="Cloud"
                >
                  <CloudIcon class="h-4 w-4" />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Cloud — browse games &amp; revisions</Tooltip.Content>
          </Tooltip.Root>

          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon"
                  onclick={() => goto('/teams')}
                  aria-label="Teams"
                >
                  <UsersIcon class="h-4 w-4" />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Teams</Tooltip.Content>
          </Tooltip.Root>

          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon"
                  onclick={() => checkUpdate(false)}
                  disabled={checkingUpdate || installingUpdate}
                  aria-label="Check for updates"
                >
                  <RefreshIcon class="h-4 w-4 {checkingUpdate ? 'animate-spin' : ''}" />
                </Button>
              {/snippet}
          </Tooltip.Trigger>
            <Tooltip.Content>Check for updates</Tooltip.Content>
          </Tooltip.Root>
        </div>
      </Tooltip.Provider>
    </div>
  </header>

  <!-- Update banner -->
  {#if updateInfo?.available}
    <Card.Root class="fade-in border-blue-500/30 bg-blue-500/5">
      <Card.Content class="flex items-start gap-4 pt-6">
        <div class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg bg-blue-500/10 text-blue-400">
          <DownloadIcon class="h-5 w-5" />
        </div>
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-3">
            <h3 class="text-base font-semibold">Update available</h3>
            <Badge variant="secondary" class="font-mono-tab">
              v{updateInfo.currentVersion} → v{updateInfo.version}
            </Badge>
          </div>
          {#if updateInfo.notes}
            <pre class="mt-3 max-h-32 overflow-y-auto whitespace-pre-wrap rounded-md border bg-background p-3 font-mono text-xs leading-relaxed text-muted-foreground">{updateInfo.notes}</pre>
          {/if}
          {#if installingUpdate && updateProgress}
            {@const pct = updateProgress.total
              ? Math.min(100, Math.round((updateProgress.downloaded / updateProgress.total) * 100))
              : null}
            <div class="mt-4">
              <div class="h-2 w-full overflow-hidden rounded-full bg-muted">
                <div class="h-full bg-blue-500 transition-all" style="width: {pct ?? 30}%"></div>
              </div>
              <div class="mt-2 font-mono-tab text-xs text-muted-foreground">
                {(updateProgress.downloaded / 1_048_576).toFixed(1)} MB
                {#if updateProgress.total}
                  / {(updateProgress.total / 1_048_576).toFixed(1)} MB ({pct}%)
                {/if}
              </div>
            </div>
          {:else}
            <Button size="lg" class="mt-4" onclick={installUpdate} disabled={busy || installingUpdate}>
              Download &amp; install
            </Button>
          {/if}
        </div>
      </Card.Content>
    </Card.Root>
  {/if}

  <!-- CA setup — only render once the status check has resolved, otherwise
       the banner flickers in for half a second every time this page mounts. -->
  {#if caState && !caState.installed}
    <Card.Root class="fade-in border-amber-500/30 bg-amber-500/5">
      <Card.Content class="flex items-start gap-4 pt-6">
        <div class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg bg-amber-500/10 text-amber-400">
          <ShieldAlertIcon class="h-5 w-5" />
        </div>
        <div class="flex-1">
          <h3 class="text-base font-semibold">Trust local HTTPS</h3>
          <p class="mt-1.5 text-sm leading-relaxed text-muted-foreground">
            Install the local Root CA so browsers trust <code class="font-mono-tab text-foreground">localhost</code>
            HTTPS. No UAC; user-scope only.
          </p>
          <Button size="lg" class="mt-4" onclick={installCa} disabled={busy}>
            <ShieldIcon />
            Install Local CA
          </Button>
        </div>
      </Card.Content>
    </Card.Root>
  {/if}

  <!-- Games -->
  <section class="flex flex-col gap-5">
    <div class="flex items-center justify-between">
      <div>
        <h2 class="text-xl font-semibold tracking-tight">Games</h2>
        <p class="mt-1 text-sm text-muted-foreground">
          {#if hasProfiles}
            Click a profile to launch. Press <span class="kbd">↵</span> to launch the active one,
            or <span class="kbd">⌘</span><span class="kbd">N</span> to add a new game.
          {:else}
            Add your first game below.
          {/if}
        </p>
      </div>
      <div class="flex items-center gap-2">
        {#if allTeams.length > 0}
          <Button
            variant="ghost"
            size="sm"
            onclick={refreshTeamsAndCatalog}
            disabled={busy || catalogLoading}
            title="Refresh team catalogues"
          >
            <RefreshIcon class={catalogLoading ? 'animate-spin' : ''} />
          </Button>
        {/if}
        {#if hasProfiles}
          <Button size="lg" onclick={openNewDraft}>
            <PlusIcon />
            New game
          </Button>
        {/if}
      </div>
    </div>

    {#if !hasProfiles}
      <Card.Root class="border-dashed">
        <Card.Content class="flex flex-col items-start gap-4 py-12">
          <div class="flex h-12 w-12 items-center justify-center rounded-xl border bg-muted text-muted-foreground">
            <FolderIcon class="h-5 w-5" />
          </div>
          <div>
            <h3 class="text-lg font-semibold">Pin your first game</h3>
            <p class="mt-1.5 max-w-md text-sm leading-relaxed text-muted-foreground">
              Point us at a math folder and a front URL. We'll keep them pinned so one click
              fires up the whole local test matrix.
            </p>
          </div>
          <Button size="lg" onclick={openNewDraft}>
            <PlusIcon />
            Add a game
          </Button>
        </Card.Content>
      </Card.Root>
    {:else}
      {#each gameGroups as g, gi (g.key)}
        <div class="flex flex-col gap-3" style="animation-delay: {gi * 60}ms;">
          <div class="flex items-center gap-2 border-b pb-2">
            {#if g.teamId === null}
              <FolderIcon class="h-4 w-4 text-muted-foreground" />
              <h3 class="text-sm font-semibold tracking-tight">Mine</h3>
              <span class="text-xs text-muted-foreground">· local only</span>
            {:else}
              <UsersRoundIcon class="h-4 w-4 text-muted-foreground" />
              <h3 class="text-sm font-semibold tracking-tight">{g.teamName}</h3>
              <Badge variant="outline" class="text-[10px]">team</Badge>
              <span class="text-xs text-muted-foreground">
                · {g.local.length} pulled{g.available.length > 0 ? `, ${g.available.length} available` : ''}
              </span>
            {/if}
          </div>

          {#each g.local as p, i (p.id)}
            {@const active = activeProfileId === p.id}
            {@const ready = readyProfiles[p.id]}
            {@const catalogEntry = catalog.find((c) => c.profile.id === p.id)}
            {@const inCatalog = catalogEntry?.profile.hasMath ?? false}
            {@const hasUpdate =
              p.teamId && catalogEntry ? catalogEntry.profile.updatedAt > p.updatedAt : false}
            {@const pushTargetId = p.teamId ?? allTeams[0]?.id ?? null}
            {@const pushTargetName = allTeams.find((t) => t.id === pushTargetId)?.name ?? null}
            {@const teamOfProfile = p.teamId
              ? allTeams.find((t) => t.id === p.teamId) ?? null
              : null}
            {@const canRemoveFromTeam = teamOfProfile?.role === 'owner'}
            <Card.Root
              class="group fade-in relative overflow-hidden transition hover:border-foreground/25 {active
                ? 'border-foreground/40'
                : ''}"
              style="animation-delay: {i * 40}ms;"
            >
              {#if active}
                <span class="absolute left-0 top-4 bottom-4 w-[3px] rounded-r bg-foreground/80"></span>
              {/if}
              <Card.Content class="flex items-center gap-4 py-4 pl-6 pr-4">
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-3 flex-wrap">
                    <button
                      type="button"
                      class="text-left text-lg font-semibold tracking-tight transition hover:text-foreground/90"
                      onclick={() => launchProfile(p)}
                      disabled={busy}
                    >
                      {p.name}
                    </button>
                    <Badge variant="secondary" class="font-mono-tab text-xs">{profileGameSlug(p)}</Badge>
                    {#if cloudRevOf(p.name) !== null}
                      <Badge variant="outline" class="gap-1 text-xs text-emerald-500 border-emerald-500/50">
                        <CloudIcon class="h-3 w-3" />
                        cloud rev {cloudRevOf(p.name)}
                      </Badge>
                    {/if}
                    {#if ready === false}
                      <Badge variant="outline" class="text-xs text-amber-500 border-amber-500/50">
                        math missing
                      </Badge>
                    {/if}
                    {#if hasUpdate}
                      <Badge variant="outline" class="text-xs text-blue-500 border-blue-500/50">
                        update available
                      </Badge>
                    {/if}
                    {#if active}
                      <Badge variant="outline" class="gap-1.5 text-xs">
                        <span class="status-dot status-dot-live"></span>
                        active
                      </Badge>
                    {/if}
                  </div>
                  <div class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground font-mono-tab">
                    <span class="flex items-center gap-1.5" title={p.gamePath}>
                      <FolderIcon class="h-3.5 w-3.5" />
                      {abbreviatePath(p.gamePath, 46)}
                    </span>
                    <span class="flex items-center gap-1.5" title={p.gameUrl}>
                      <MonitorIcon class="h-3.5 w-3.5" />
                      {p.gameUrl}
                    </span>
                    <span>· updated {formatRelative(p.updatedAt)}</span>
                  </div>
                </div>

                <div class="flex items-center gap-1">
                  <Tooltip.Provider delayDuration={200}>
                    {#if p.teamId && ready !== false}
                      <Tooltip.Root>
                        <Tooltip.Trigger>
                          {#snippet child({ props })}
                            <Button
                              {...props}
                              variant={hasUpdate ? 'default' : 'ghost'}
                              size="icon"
                              class={hasUpdate
                                ? 'opacity-100'
                                : 'opacity-0 transition group-hover:opacity-100'}
                              onclick={() => refreshFromTeam(p)}
                              disabled={busy}
                            >
                              <DownloadCloudIcon />
                            </Button>
                          {/snippet}
                        </Tooltip.Trigger>
                        <Tooltip.Content>
                          {hasUpdate ? 'Pull latest — update available' : 'Pull latest from team'}
                        </Tooltip.Content>
                      </Tooltip.Root>
                    {/if}

                    {#if ready !== false}
                      <Tooltip.Root>
                        <Tooltip.Trigger>
                          {#snippet child({ props })}
                            <Button
                              {...props}
                              variant="ghost"
                              size="icon"
                              class="opacity-0 transition group-hover:opacity-100"
                              onclick={() => openShareDialog(p)}
                              disabled={busy}
                            >
                              <Share2Icon />
                            </Button>
                          {/snippet}
                        </Tooltip.Trigger>
                        <Tooltip.Content>Share preview link…</Tooltip.Content>
                      </Tooltip.Root>
                    {/if}

                    {#if canRemoveFromTeam && teamOfProfile}
                      <Tooltip.Root>
                        <Tooltip.Trigger>
                          {#snippet child({ props })}
                            <Button
                              {...props}
                              variant="ghost"
                              size="icon"
                              class="text-destructive opacity-0 transition hover:text-destructive group-hover:opacity-100"
                              onclick={() => removeFromCatalog(p)}
                              disabled={busy}
                            >
                              <UsersRoundIcon class="h-4 w-4" />
                            </Button>
                          {/snippet}
                        </Tooltip.Trigger>
                        <Tooltip.Content>
                          Remove from team "{teamOfProfile.name}" (owner)
                        </Tooltip.Content>
                      </Tooltip.Root>
                    {/if}

                    {#if pushTargetId && pushTargetName && ready !== false}
                      <Tooltip.Root>
                        <Tooltip.Trigger>
                          {#snippet child({ props })}
                            <Button
                              {...props}
                              variant="ghost"
                              size="icon"
                              class="opacity-0 transition group-hover:opacity-100"
                              onclick={() => onPushClick(p)}
                              disabled={busy}
                            >
                              <UploadIcon />
                            </Button>
                          {/snippet}
                        </Tooltip.Trigger>
                        <Tooltip.Content>
                          {#if p.teamId}
                            Push my changes to "{pushTargetName}"
                          {:else if allTeams.length > 1}
                            Share… (pick team)
                          {:else}
                            Share with "{pushTargetName}"
                          {/if}
                        </Tooltip.Content>
                      </Tooltip.Root>
                    {/if}

                    <Tooltip.Root>
                      <Tooltip.Trigger>
                        {#snippet child({ props })}
                          <Button
                            {...props}
                            variant="ghost"
                            size="icon"
                            class="opacity-0 transition group-hover:opacity-100"
                            onclick={() => openEditDraft(p)}
                            disabled={busy}
                          >
                            <PencilIcon />
                          </Button>
                        {/snippet}
                      </Tooltip.Trigger>
                      <Tooltip.Content>Edit</Tooltip.Content>
                    </Tooltip.Root>

                    <Tooltip.Root>
                      <Tooltip.Trigger>
                        {#snippet child({ props })}
                          <Button
                            {...props}
                            variant="ghost"
                            size="icon"
                            class="text-destructive opacity-0 transition hover:text-destructive group-hover:opacity-100"
                            onclick={() => deleteProfile(p)}
                            disabled={busy}
                          >
                            <TrashIcon />
                          </Button>
                        {/snippet}
                      </Tooltip.Trigger>
                      <Tooltip.Content>Delete</Tooltip.Content>
                    </Tooltip.Root>
                  </Tooltip.Provider>

                  {#if ready === false && inCatalog}
                    <Button size="lg" onclick={() => pullMissingMath(p)} disabled={busy}>
                      <DownloadCloudIcon class="h-4 w-4" />
                      Pull
                    </Button>
                  {:else if ready === false}
                    <Button
                      size="lg"
                      variant="outline"
                      disabled
                      title="Math files missing at {p.gamePath}"
                    >
                      Missing math
                    </Button>
                  {:else}
                    <Button
                      size="lg"
                      onclick={() => launchProfile(p)}
                      disabled={busy}
                    >
                      <PlayIcon class="h-4 w-4" />
                      Launch
                    </Button>
                  {/if}
                </div>
              </Card.Content>
            </Card.Root>
          {/each}

          {#each g.available as a (a.tp.id)}
            <Card.Root class="fade-in border-dashed">
              <Card.Content class="flex items-center gap-4 py-4 pl-6 pr-4">
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-3 flex-wrap">
                    <span class="text-lg font-semibold tracking-tight">{a.tp.name}</span>
                    <Badge variant="secondary" class="font-mono-tab text-xs">{a.tp.gameSlug}</Badge>
                    <Badge variant="outline" class="text-xs">not pulled</Badge>
                  </div>
                  <div class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground font-mono-tab">
                    <span class="flex items-center gap-1.5">
                      <MonitorIcon class="h-3.5 w-3.5" />
                      {a.tp.gameUrl || '—'}
                    </span>
                    <span>· updated {formatRelative(a.tp.updatedAt)}</span>
                  </div>
                </div>
                <Button size="lg" onclick={() => pullTeamProfile(a.teamId, a.tp)} disabled={busy}>
                  <DownloadCloudIcon class="h-4 w-4" />
                  Pull
                </Button>
              </Card.Content>
            </Card.Root>
          {/each}
        </div>
      {/each}
    {/if}
  </section>

  <!-- Resolutions -->
  <section class="flex flex-col gap-4">
    <div class="flex items-center justify-between">
      <div>
        <h2 class="text-xl font-semibold tracking-tight">Resolutions</h2>
        <p class="mt-1 text-sm text-muted-foreground">
          <span class="font-mono-tab text-foreground">{enabledResCount}/{resolutions.length}</span> enabled ·
          applied per launch.
        </p>
      </div>
      <div class="flex items-center gap-2">
        {#if activeProfileId}
          <Button variant="outline" size="lg" onclick={saveResolutionsToActiveProfile} disabled={busy}>
            Snapshot to profile
          </Button>
        {/if}
        <Button variant="ghost" size="lg" onclick={() => (showResolutions = !showResolutions)}>
          {showResolutions ? 'Hide' : 'Show'}
          <ChevronDownIcon class="transition {showResolutions ? 'rotate-180' : ''}" />
        </Button>
      </div>
    </div>

    {#if showResolutions}
      <Card.Root class="fade-in">
        <Card.Content class="grid grid-cols-1 gap-1 p-2 sm:grid-cols-2">
          {#each resolutions as r (r.id)}
            <label
              for="res-{r.id}"
              class="group flex cursor-pointer items-center gap-3 rounded-md px-3 py-3 transition hover:bg-muted/50"
            >
              <Checkbox
                id="res-{r.id}"
                checked={r.enabled}
                onCheckedChange={(v) => toggleResolution(r.id, Boolean(v))}
                disabled={busy}
              />
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <span class="text-sm font-medium">{r.label}</span>
                  {#if !r.builtin}
                    <Badge variant="outline" class="text-[10px]">custom</Badge>
                  {/if}
                </div>
                <div class="font-mono-tab text-xs text-muted-foreground">
                  {r.width} × {r.height}
                </div>
              </div>
              {#if !r.builtin}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="text-destructive opacity-0 transition group-hover:opacity-100"
                  onclick={(e) => {
                    e.preventDefault();
                    deleteCustomResolution(r.id);
                  }}
                  disabled={busy}
                >
                  <TrashIcon />
                </Button>
              {/if}
            </label>
          {/each}
        </Card.Content>
      </Card.Root>
    {/if}
  </section>

  <!-- Footer -->
  <footer class="mt-auto flex flex-wrap items-center justify-between gap-6 border-t pt-6 text-sm">
    <div class="flex items-center gap-3">
      <Switch id="close-after" bind:checked={closeAfterLaunch} onCheckedChange={persistCloseAfter} />
      <Label for="close-after" class="flex items-center gap-2 text-sm font-normal">
        <MinimizeIcon class="h-4 w-4 text-muted-foreground" />
        Minimise window after launching
      </Label>
    </div>

    <div class="flex items-center gap-4 text-muted-foreground">
      {#if caState?.installed}
        <span class="flex items-center gap-2">
          <ShieldIcon class="h-4 w-4 text-emerald-500" />
          <span>CA trusted</span>
          <button
            type="button"
            class="underline-offset-4 hover:text-foreground hover:underline"
            onclick={uninstallCa}
            disabled={busy}
          >
            remove
          </button>
        </span>
      {/if}
    </div>
  </footer>
</main>

<!-- Share preview dialog -->
<Dialog.Root bind:open={shareOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2">
        <GlobeIcon class="h-5 w-5" />
        Share a preview link
      </Dialog.Title>
      <Dialog.Description>
        {#if shareProfile}
          Publishes "{shareProfile.name}" as a static page on
          <span class="font-mono-tab">github.io</span>. The math + game run in the browser via WASM
          — no server needed. Anyone with the URL can play.
        {/if}
      </Dialog.Description>
    </Dialog.Header>

    {#if !githubUser}
      <!-- Sign-in CTA: previews need a GitHub repo to host the bundle.
           Surfaces auth here so users don't have to detour through Teams. -->
      <div class="my-4 flex flex-col items-start gap-3 rounded-md border bg-card/50 p-4">
        <div class="flex items-center gap-2">
          <KeyRoundIcon class="h-5 w-5" />
          <p class="text-sm font-semibold">Sign in to GitHub to publish</p>
        </div>
        <p class="text-xs text-muted-foreground">
          Each preview lives in a public GitHub repo under your account. Sign in once,
          then publish/unpublish from here without leaving this dialog.
        </p>
        <Button onclick={() => (signInOpen = true)} disabled={shareBusy}>
          <LogInIcon class="h-4 w-4" />
          Sign in with GitHub
        </Button>
      </div>
    {/if}

    <div class="my-4 flex flex-col gap-4" class:opacity-50={!githubUser} class:pointer-events-none={!githubUser}>
      <div class="flex flex-col gap-2">
        <Label for="frontPath">Built front folder</Label>
        <div class="flex gap-2">
          <Input
            id="frontPath"
            bind:value={shareFrontPath}
            placeholder="C:\path\to\game-project\dist"
            readonly
          />
          <Button variant="outline" onclick={pickShareFolder} disabled={shareBusy}>
            <FolderIcon />
            Browse…
          </Button>
        </div>
        <p class="text-xs text-muted-foreground">
          Folder containing the production build of your slot front (with <code
            class="font-mono-tab">index.html</code
          > at the root).
        </p>
      </div>

      <div class="flex flex-col gap-2">
        <Label>Math payload</Label>
        <label
          class="flex cursor-pointer items-start gap-3 rounded-md border p-3 hover:bg-accent {shareMathMode ===
          'sampled'
            ? 'border-foreground/40 bg-accent/40'
            : ''}"
        >
          <input
            type="radio"
            name="mathMode"
            value="sampled"
            bind:group={shareMathMode}
            class="mt-1"
          />
          <div class="flex-1 text-sm">
            <div class="font-medium">Sampled (recommended)</div>
            <p class="mt-0.5 text-xs text-muted-foreground">
              ~100 books per mode with a curated payout distribution (50% no-wins +
              max-win + average + tier spread). Tiny output, fastest publish,
              full-fidelity per-book animations. Limited variety — best for a quick
              demo link.
            </p>
          </div>
        </label>
        <label
          class="flex cursor-pointer items-start gap-3 rounded-md border p-3 hover:bg-accent {shareMathMode ===
          'partial'
            ? 'border-foreground/40 bg-accent/40'
            : ''}"
        >
          <input
            type="radio"
            name="mathMode"
            value="partial"
            bind:group={shareMathMode}
            class="mt-1"
          />
          <div class="flex-1 text-sm">
            <div class="font-medium">Partial</div>
            <p class="mt-0.5 text-xs text-muted-foreground">
              All books, but each has half its events truncated. Animations + RTP are
              intentionally broken so the production math stays harder to
              reverse-engineer. Slow to publish for big games.
            </p>
          </div>
        </label>
        <label
          class="flex cursor-pointer items-start gap-3 rounded-md border p-3 hover:bg-accent {shareMathMode ===
          'full'
            ? 'border-foreground/40 bg-accent/40'
            : ''}"
        >
          <input
            type="radio"
            name="mathMode"
            value="full"
            bind:group={shareMathMode}
            class="mt-1"
          />
          <div class="flex-1 text-sm">
            <div class="font-medium">Full</div>
            <p class="mt-0.5 text-xs text-muted-foreground">
              Math files shipped as-is. Preview is faithful (RTP, pacing, animations)
              but the math is <span class="text-amber-500"
                >fully public on github.io</span
              > — anyone with the URL can read the weights and books.
            </p>
          </div>
        </label>
        <p class="text-xs text-muted-foreground">
          Whichever mode you pick, anything served via GitHub Pages is reachable by
          anyone — there's no truly private preview link without a backend.
        </p>
      </div>

      {#if shareUrl}
        <div class="rounded-md border border-emerald-500/30 bg-emerald-500/5 p-3">
          <div class="text-xs text-muted-foreground mb-1.5">Live URL</div>
          <div class="flex items-center gap-2">
            <code class="font-mono-tab flex-1 break-all text-sm">{shareUrl}</code>
            <Button size="icon" variant="ghost" onclick={copyShareUrl}>
              <CopyIcon class="h-4 w-4" />
            </Button>
            <Button size="icon" variant="ghost" onclick={() => openUrl(shareUrl ?? '')}>
              <ExternalLinkIcon class="h-4 w-4" />
            </Button>
          </div>
          <p class="mt-2 text-xs text-muted-foreground">
            GitHub Pages takes ~30-60 sec to propagate the first time. Refresh if you get a 404.
          </p>
        </div>
      {/if}
    </div>

    <Dialog.Footer>
      {#if shareUrl}
        <Button variant="ghost" class="text-destructive" onclick={unpublishShare} disabled={shareBusy}>
          <TrashIcon />
          Unpublish
        </Button>
      {/if}
      <Button variant="outline" onclick={() => (shareOpen = false)}>Close</Button>
      <Button onclick={publishShare} disabled={shareBusy || !shareFrontPath.trim() || !githubUser}>
        <Share2Icon />
        {shareUrl ? 'Re-publish' : 'Publish'}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Push picker: shown when sharing a local profile and user is in >1 team -->
<Dialog.Root bind:open={pushPickerOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Share with which team?</Dialog.Title>
      <Dialog.Description>
        {#if pushPickerProfile}
          "{pushPickerProfile.name}" will be uploaded to the team you pick
          (profile + math + saved rounds).
        {/if}
      </Dialog.Description>
    </Dialog.Header>
    <div class="my-4 flex flex-col gap-2">
      {#each allTeams as t (t.id)}
        <button
          type="button"
          class="flex items-center justify-between rounded-md border px-4 py-3 text-left hover:bg-accent"
          onclick={() => {
            const p = pushPickerProfile;
            pushPickerOpen = false;
            pushPickerProfile = null;
            if (p) pushProfileToTeam(p, t.id);
          }}
          disabled={busy}
        >
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <span class="font-medium">{t.name}</span>
              {#if t.role === 'owner'}
                <Badge variant="secondary" class="text-[10px]">owner</Badge>
              {/if}
            </div>
            <div class="font-mono-tab text-xs text-muted-foreground">
              {t.slug}
            </div>
          </div>
          <UploadIcon class="h-4 w-4 text-muted-foreground" />
        </button>
      {/each}
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (pushPickerOpen = false)}>Cancel</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Drawer: create/edit profile -->
<Sheet.Root bind:open={drawerOpen}>
  <Sheet.Content side="right" class="w-full !max-w-[480px] flex flex-col gap-0 p-0">
    {#if draft}
      <Sheet.Header class="border-b px-6 py-5">
        <Sheet.Title class="text-xl">
          {draft.mode === 'edit' ? 'Edit profile' : 'Add a game'}
        </Sheet.Title>
        <Sheet.Description>
          {draft.mode === 'edit'
            ? 'Update the math folder, URL or name.'
            : 'Point us at a math folder and a front URL. We\'ll pin it.'}
        </Sheet.Description>
      </Sheet.Header>

      <div class="flex-1 overflow-y-auto px-6 py-5 flex flex-col gap-6">
        <div class="flex flex-col gap-2">
          <Label for="draft-name" class="text-sm">Game name</Label>
          <Input
            id="draft-name"
            type="text"
            bind:value={draft.name}
            placeholder="e.g. dice-drop"
            class="h-10"
          />
        </div>

        <div class="flex flex-col gap-2">
          <div class="flex items-center justify-between">
            <Label for="draft-path" class="text-sm">Math folder</Label>
            {#if draft.game}
              <span class="font-mono-tab text-xs text-muted-foreground">
                {draft.game.modes.length} mode{draft.game.modes.length === 1 ? '' : 's'}
              </span>
            {/if}
          </div>
          <div class="flex gap-2">
            <Input
              id="draft-path"
              type="text"
              value={draft.game?.gamePath ?? ''}
              readonly
              placeholder="No folder selected"
              class="h-10 font-mono-tab text-sm"
            />
            <Button variant="outline" size="lg" onclick={pickDraftFolder} disabled={busy}>
              <FolderIcon />
              Browse
            </Button>
          </div>
          {#if draft.game}
            <div class="mt-1 flex flex-wrap gap-1.5">
              <Badge variant="secondary" class="font-mono-tab text-xs">{draft.game.slug}</Badge>
              {#each draft.game.modes as m (m)}
                <Badge variant="outline" class="font-mono-tab text-xs">{m}</Badge>
              {/each}
            </div>
          {/if}
        </div>

        <div class="flex flex-col gap-2">
          <Label for="draft-url" class="text-sm">Front URL</Label>
          <Input
            id="draft-url"
            type="url"
            bind:value={draft.gameUrl}
            placeholder="http://localhost:5174"
            class="h-10 font-mono-tab text-sm"
          />
          <p class="text-xs text-muted-foreground">
            Your game's frontend dev server — usually a Vite URL.
          </p>
        </div>

        <Separator />

        <div class="rounded-lg border bg-muted/30 p-4">
          <div class="text-sm">
            Saving will pin
            <span class="font-mono-tab font-semibold">{enabledResCount}/{resolutions.length}</span>
            resolutions onto this profile. Re-snapshot any time from the main view.
          </div>
        </div>
      </div>

      <Sheet.Footer class="flex-row justify-end gap-2 border-t px-6 py-4">
        <Button variant="outline" size="lg" onclick={closeDraft}>Cancel</Button>
        <Button
          size="lg"
          onclick={saveDraft}
          disabled={busy || !draft.game || !draft.name.trim() || !draft.gameUrl.trim()}
        >
          {draft.mode === 'edit' ? 'Save changes' : 'Create profile'}
        </Button>
      </Sheet.Footer>
    {/if}
  </Sheet.Content>
</Sheet.Root>

<!-- GitHub sign-in dialog (shared, opened from topbar or Share preview) -->
<GithubSignInDialog bind:open={signInOpen} onSignedIn={onGithubSignedIn} />
<CloudSignInDialog bind:open={cloudSignInOpen} onSignedIn={onCloudSignedIn} />

<!-- LGS port settings dialog -->
<Dialog.Root bind:open={portSettingsOpen}>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>LGS port</Dialog.Title>
      <Dialog.Description>
        TCP port the local game server binds to (1024-65535). Changing it while
        the LGS is running will restart it on the new port.
      </Dialog.Description>
    </Dialog.Header>
    <div class="my-4 space-y-2">
      <Label for="lgsPortInput">Port</Label>
      <Input
        id="lgsPortInput"
        type="number"
        min={1024}
        max={65535}
        bind:value={lgsPort}
        class="font-mono-tab"
      />
      <p class="text-xs text-muted-foreground">
        Default: <span class="font-mono-tab">{DEFAULT_PORT}</span>. The test view URL
        will be <span class="font-mono-tab">https://localhost:{lgsPort}/test/</span>.
      </p>
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (portSettingsOpen = false)}>Cancel</Button>
      <Button onclick={() => applyLgsPortAndRestart(lgsPort)}>
        {status.running ? 'Save & restart LGS' : 'Save'}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
