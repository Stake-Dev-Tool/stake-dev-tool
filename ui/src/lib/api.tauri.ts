// Tauri-bound API surface.
//
// Every client here reaches the desktop shell through `@tauri-apps` (invoke,
// dialog, updater, process). Importing this module pulls Tauri into the chunk,
// so it must only be reached by the desktop-chrome routes — never by `/test`.
// Shared, Tauri-free data types live in `./api.http` and are imported below.

import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import type { PrepareSession, ResolutionPreset, Settings } from './api.http';

export type LgsStatus = {
  running: boolean;
  bound_addr: string | null;
  math_dir: string | null;
};

export type GameInfo = {
  slug: string;
  path: string;
  modes: string[];
};

export type InspectedGame = {
  slug: string;
  gamePath: string;
  mathDir: string;
  modes: string[];
};

export type CaStatus = {
  installed: boolean;
  caPath: string;
};

export type LaunchOptions = {
  gameUrl: string;
  gameSlug: string;
  lang?: string;
  currency?: string;
  device?: string;
  social?: boolean;
  extraParams?: Array<[string, string]>;
};

export const lgs = {
  status: () => invoke<LgsStatus>('lgs_status'),
  start: (port: number, mathDir: string) =>
    invoke<LgsStatus>('start_lgs', { port, mathDir }),
  stop: () => invoke<LgsStatus>('stop_lgs'),
  listGames: (mathDir: string) => invoke<GameInfo[]>('list_games', { mathDir }),
  inspect: (path: string) => invoke<InspectedGame>('inspect_game_folder', { path }),
  launch: (options: LaunchOptions) => invoke<string>('launch_game', { options }),
  buildUrl: (options: LaunchOptions) => invoke<string>('build_launch_url', { options })
};

export const ca = {
  status: () => invoke<CaStatus>('ca_status'),
  install: () => invoke<CaStatus>('install_ca'),
  uninstall: () => invoke<CaStatus>('uninstall_ca')
};

export const sessions = {
  prepare: (payload: PrepareSession) => invoke<void>('prepare_session', { payload })
};

export type OpenBrowserResult = { method: string; url: string };

export const browser = {
  openTest: (url: string) => invoke<OpenBrowserResult>('open_test_browser', { url })
};

// ===== Updater =====
// Thin wrappers around @tauri-apps/plugin-updater so the main page can show
// update status without pulling the plugin API everywhere.

export type UpdateInfo = {
  available: boolean;
  currentVersion: string;
  version?: string;
  notes?: string;
};

export async function checkForUpdates(): Promise<UpdateInfo> {
  const { check } = await import('@tauri-apps/plugin-updater');
  const { getVersion } = await import('@tauri-apps/api/app');
  const currentVersion = await getVersion();
  const update = await check();
  if (!update) return { available: false, currentVersion };
  return {
    available: true,
    currentVersion,
    version: update.version,
    notes: update.body
  };
}

export async function downloadAndInstallUpdate(
  onProgress?: (downloaded: number, total?: number) => void
): Promise<void> {
  const { check } = await import('@tauri-apps/plugin-updater');
  const { relaunch } = await import('@tauri-apps/plugin-process');
  const update = await check();
  if (!update) throw new Error('No update available');
  let downloaded = 0;
  let total: number | undefined;
  await update.downloadAndInstall((event) => {
    if (event.event === 'Started') {
      total = event.data.contentLength ?? undefined;
    } else if (event.event === 'Progress') {
      downloaded += event.data.chunkLength;
      onProgress?.(downloaded, total);
    }
  });
  await relaunch();
}

export type Profile = {
  id: string;
  name: string;
  gamePath: string;
  gameUrl: string;
  gameSlug: string;
  resolutions: ResolutionPreset[];
  createdAt: number;
  updatedAt: number;
  teamId?: string | null;
};

export type SaveProfilePayload = {
  id?: string | null;
  name: string;
  gamePath: string;
  gameUrl: string;
  gameSlug: string;
  resolutions?: ResolutionPreset[];
};

export const profiles = {
  list: () => invoke<Profile[]>('list_profiles'),
  save: (payload: SaveProfilePayload) => invoke<Profile>('save_profile', { payload }),
  remove: (id: string) => invoke<void>('delete_profile', { id })
};

// ===== Settings (resolutions) =====

// Tauri-side client (used by the desktop main page)
export const settings = {
  get: () => invoke<Settings>('get_settings'),
  toggle: (id: string, enabled: boolean) =>
    invoke<Settings>('toggle_resolution', { id, enabled }),
  addCustom: (label: string, width: number, height: number) =>
    invoke<Settings>('add_custom_resolution', { label, width, height }),
  deleteCustom: (id: string) => invoke<Settings>('delete_custom_resolution', { id }),
  replace: (resolutions: ResolutionPreset[]) =>
    invoke<Settings>('replace_resolutions', { resolutions })
};

// ===== GitHub auth + Teams =====

export type GithubUser = {
  id: number;
  login: string;
  name?: string | null;
  avatar_url?: string | null;
};

export type DeviceCode = {
  device_code: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
};

export type AuthState = { user: GithubUser };

export type DeviceFlowPoll = {
  auth: AuthState | null;
  next_interval_secs: number;
};

export type GithubOrg = {
  login: string;
  id: number;
  avatar_url?: string | null;
  description?: string | null;
};

export const githubAuth = {
  currentUser: () => invoke<GithubUser | null>('github_current_user'),
  startDeviceFlow: () => invoke<DeviceCode>('github_start_device_flow'),
  pollDeviceFlow: (deviceCode: string, currentInterval: number) =>
    invoke<DeviceFlowPoll>('github_poll_device_flow', { deviceCode, currentInterval }),
  logout: () => invoke<void>('github_logout'),
  listOrgs: () => invoke<GithubOrg[]>('github_list_orgs')
};

export type TeamRole = 'owner' | 'member';

export type Team = {
  id: string;
  name: string;
  repoOwner: string;
  repoName: string;
  role: TeamRole;
  htmlUrl: string;
  addedAt: number;
  lastSyncAt?: number | null;
};

export type DiscoveredTeam = {
  teamId: string;
  teamName: string;
  repoOwner: string;
  repoName: string;
  htmlUrl: string;
};

export type SyncReport = {
  profilesPushed: number;
  profilesPulled: number;
  roundsPushed: number;
  roundsPulled: number;
};

export type TeamProfileInfo = {
  id: string;
  name: string;
  gameSlug: string;
  gameUrl: string;
  hasMath: boolean;
  updatedAt: number;
};

export type CatalogEntry = {
  teamId: string;
  teamName: string;
  profile: TeamProfileInfo;
};

export type MathSyncReport = {
  filesUploaded: number;
  filesSkipped: number;
  chunksUploaded: number;
  bytesUploaded: number;
};

export type PublishReport = {
  url: string;
  filesUploaded: number;
  filesSkipped: number;
  bytesUploaded: number;
};

/// `sampled` keeps ~100 books per mode with a curated payout distribution
/// (no-wins + max + average + tier spread). Tiny payload, fast publish,
/// limited variety. `partial` halves the events inside every book —
/// playable but RTP-broken. `full` ships math as-is.
export type MathMode = 'full' | 'partial' | 'sampled';

export const teamsApi = {
  list: () => invoke<Team[]>('teams_list'),
  active: () => invoke<Team | null>('teams_active'),
  setActive: (teamId: string | null) => invoke<void>('teams_set_active', { teamId }),
  create: (name: string, org?: string | null) =>
    invoke<Team>('teams_create', { name, org: org ?? null }),
  join: (owner: string, name: string) => invoke<Team>('teams_join', { owner, name }),
  leave: (teamId: string) => invoke<void>('teams_leave', { teamId }),
  delete: (teamId: string) => invoke<void>('teams_delete', { teamId }),
  invite: (teamId: string, username: string) =>
    invoke<void>('teams_invite', { teamId, username }),
  discover: () => invoke<DiscoveredTeam[]>('teams_discover'),
  sync: (teamId: string) => invoke<SyncReport>('teams_sync', { teamId }),
  pushMath: (teamId: string, gameSlug: string, gamePath: string) =>
    invoke<MathSyncReport>('teams_push_math', { teamId, gameSlug, gamePath }),
  pullMath: (teamId: string, gameSlug: string, destPath: string) =>
    invoke<MathSyncReport>('teams_pull_math', { teamId, gameSlug, destPath }),
  listRemoteGames: (teamId: string) =>
    invoke<string[]>('teams_list_remote_games', { teamId }),
  defaultMathRoot: (teamId: string) =>
    invoke<string>('teams_default_math_root', { teamId }),
  publishPreview: (profileId: string, frontPath: string, mathMode: MathMode) =>
    invoke<PublishReport>('preview_publish', { profileId, frontPath, mathMode }),
  unpublishPreview: (profileId: string) =>
    invoke<void>('preview_unpublish', { profileId }),
  buildLocalPreview: (profileId: string, frontPath: string, mathMode: MathMode) =>
    invoke<string>('preview_build_local', { profileId, frontPath, mathMode }),
  listProfiles: (teamId: string) =>
    invoke<TeamProfileInfo[]>('teams_list_profiles', { teamId }),
  pullProfile: (teamId: string, teamProfileId: string) =>
    invoke<Profile>('teams_pull_profile', { teamId, teamProfileId }),
  pushProfile: (teamId: string, profileId: string) =>
    invoke<void>('teams_push_profile', { teamId, profileId }),
  allCatalogs: () => invoke<CatalogEntry[]>('teams_all_catalogs'),
  removeFromCatalog: (teamId: string, profileId: string) =>
    invoke<void>('teams_remove_from_catalog', { teamId, profileId })
};

export async function pickFolder(title = 'Select math root folder'): Promise<string | null> {
  const result = await openDialog({
    title,
    directory: true,
    multiple: false
  });
  if (!result) return null;
  return Array.isArray(result) ? result[0] : result;
}
