//! DEPRECATED (M3): the legacy GitHub-repo "teams" system.
//!
//! M3 replaces team sync with cloud workspaces (`crate::cloud::{workspaces,
//! sync, documents, math}`); the `teams_*` command bodies now delegate there.
//! This module is retained only so migration (`crate::cloud::migrate`) can still
//! *read* a GitHub team, and so the Teams screen can list legacy teams offering
//! a "Migrate to cloud" button. It is slated for removal post-V2. The
//! module-level `allow(dead_code)` covers the now-unused GitHub sync/catalogue
//! paths that migration does not touch.
#![allow(dead_code)]

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use uuid::Uuid;

use crate::github::api::{GithubClient, RepoInfo};
use crate::profiles::{self, Profile};

/// Repo topic used to discover team repos for the authenticated user.
pub const TEAM_MARKER_TOPIC: &str = "stake-dev-tool-team";

/// Schema version for the team repo layout. Bump when introducing breaking
/// changes to the repo file structure.
pub const TEAM_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TeamRole {
    Owner,
    Member,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    #[serde(rename = "repoOwner")]
    pub repo_owner: String,
    #[serde(rename = "repoName")]
    pub repo_name: String,
    pub role: TeamRole,
    #[serde(rename = "htmlUrl")]
    pub html_url: String,
    #[serde(rename = "addedAt")]
    pub added_at: u64,
    #[serde(default, rename = "lastSyncAt")]
    pub last_sync_at: Option<u64>,
    /// Set once this legacy team has been migrated to a cloud workspace; the
    /// value is the destination workspace id. Drives the Teams-screen "migrated"
    /// badge and disables re-migration.
    #[serde(default, rename = "migratedTo")]
    pub migrated_to: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TeamsFile {
    #[serde(default, rename = "activeTeamId")]
    active_team_id: Option<String>,
    #[serde(default)]
    teams: Vec<Team>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "teamId")]
    pub team_id: String,
    #[serde(rename = "teamName")]
    pub team_name: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}

/// Summary of a profile shared in the team repo, usable as a catalogue entry
/// on the main page. `hasMath` tells the UI whether the math files are
/// available to pull (a manifest exists in the team repo).
#[derive(Debug, Clone, Serialize)]
pub struct TeamProfileInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    #[serde(rename = "gameUrl")]
    pub game_url: String,
    #[serde(rename = "hasMath")]
    pub has_math: bool,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    #[serde(rename = "profilesPushed")]
    pub profiles_pushed: u32,
    #[serde(rename = "profilesPulled")]
    pub profiles_pulled: u32,
    #[serde(rename = "roundsPushed")]
    pub rounds_pushed: u32,
    #[serde(rename = "roundsPulled")]
    pub rounds_pulled: u32,
}

fn teams_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("could not resolve local data dir"))?
        .join("stake-dev-tool");
    Ok(dir.join("teams.json"))
}

async fn load() -> Result<TeamsFile> {
    let path = teams_path()?;
    if !fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(TeamsFile::default());
    }
    let bytes = fs::read(&path).await.context("read teams.json")?;
    let parsed: TeamsFile = serde_json::from_slice(&bytes).context("parse teams.json")?;
    Ok(parsed)
}

async fn save(file: &TeamsFile) -> Result<()> {
    let path = teams_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .context("create teams dir")?;
    }
    let bytes = serde_json::to_vec_pretty(file).context("serialize teams")?;
    fs::write(&path, bytes).await.context("write teams.json")?;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn slugify(name: &str) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse runs of dashes.
    let mut out = String::with_capacity(base.len());
    let mut last_dash = false;
    for ch in base.chars() {
        if ch == '-' {
            if !last_dash {
                out.push(ch);
            }
            last_dash = true;
        } else {
            out.push(ch);
            last_dash = false;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "team".to_string()
    } else {
        trimmed
    }
}

fn repo_name_for(name: &str) -> String {
    format!("stake-dev-tool-team-{}", slugify(name))
}

/// Default on-disk location for math pulled from a team. Laid out as
/// `<documents>/stake-dev-tool/teams/<repo-owner>_<repo-name>/` so two teams
/// with similar names don't collide.
pub fn default_team_math_root(team: &Team) -> Result<PathBuf> {
    let docs = dirs::document_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow!("could not resolve documents directory"))?;
    let safe = format!("{}_{}", team.repo_owner, team.repo_name);
    Ok(docs.join("stake-dev-tool").join("teams").join(safe))
}

pub async fn default_math_root_for(team_id: &str) -> Result<PathBuf> {
    let team = {
        let f = load().await?;
        f.teams
            .into_iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("team not found"))?
    };
    default_team_math_root(&team)
}

pub async fn list_local() -> Result<Vec<Team>> {
    let mut f = load().await?;
    f.teams.sort_by_key(|t| std::cmp::Reverse(t.added_at));
    Ok(f.teams)
}

pub async fn active_team() -> Result<Option<Team>> {
    let f = load().await?;
    let Some(id) = f.active_team_id else {
        return Ok(None);
    };
    Ok(f.teams.into_iter().find(|t| t.id == id))
}

pub async fn set_active(team_id: Option<&str>) -> Result<()> {
    let mut f = load().await?;
    f.active_team_id = team_id.map(|s| s.to_string());
    save(&f).await
}

/// Looks up one legacy team by id (used by migration and the Teams screen).
pub async fn team_by_id(team_id: &str) -> Result<Team> {
    load()
        .await?
        .teams
        .into_iter()
        .find(|t| t.id == team_id)
        .ok_or_else(|| anyhow!("team not found"))
}

/// Stamps a legacy team as migrated to `workspace_id`.
pub async fn mark_migrated(team_id: &str, workspace_id: &str) -> Result<()> {
    let mut f = load().await?;
    if let Some(t) = f.teams.iter_mut().find(|t| t.id == team_id) {
        t.migrated_to = Some(workspace_id.to_string());
    }
    save(&f).await
}

async fn upsert_local(team: Team) -> Result<Team> {
    let mut f = load().await?;
    if let Some(existing) = f.teams.iter_mut().find(|t| t.id == team.id) {
        *existing = team.clone();
    } else {
        f.teams.push(team.clone());
    }
    if f.active_team_id.is_none() {
        f.active_team_id = Some(team.id.clone());
    }
    save(&f).await?;
    Ok(team)
}

pub async fn remove_local(team_id: &str) -> Result<()> {
    let mut f = load().await?;
    let before = f.teams.len();
    f.teams.retain(|t| t.id != team_id);
    if f.teams.len() == before {
        return Err(anyhow!("team not found"));
    }
    if f.active_team_id.as_deref() == Some(team_id) {
        f.active_team_id = f.teams.first().map(|t| t.id.clone());
    }
    save(&f).await
}

/// Delete the team's GitHub repo permanently, then clear the local entry.
/// Only meaningful for owners — GitHub returns 403 otherwise.
pub async fn delete_team(team_id: &str) -> Result<()> {
    let team = {
        let f = load().await?;
        f.teams
            .into_iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("team not found"))?
    };
    if team.role != TeamRole::Owner {
        return Err(anyhow!("only the owner can delete a team"));
    }
    let client = GithubClient::from_stored_token()?;
    client
        .delete_repo(&team.repo_owner, &team.repo_name)
        .await
        .map_err(|e| {
            // Translate the most common failure into actionable guidance. The
            // `delete_repo` scope was added after the initial auth grant, so
            // users signed in before this version will hit 403.
            let s = format!("{e:#}");
            if s.contains("403") {
                anyhow!(
                    "GitHub refused the delete (403). This usually means your \
                     authorization is missing the `delete_repo` scope. \
                     Sign out and sign back in, then retry."
                )
            } else {
                e
            }
        })?;
    remove_local(team_id).await
}

pub async fn create_team(name: String, org: Option<String>) -> Result<Team> {
    if name.trim().is_empty() {
        return Err(anyhow!("team name is required"));
    }

    let client = GithubClient::from_stored_token()?;
    let repo_name = repo_name_for(&name);
    let description = format!("stake-dev-tool team workspace: {name}");
    let org_ref = org.as_deref().filter(|s| !s.is_empty());

    let repo = client
        .create_private_repo(org_ref, &repo_name, &description)
        .await?;
    tracing::info!(repo = %repo.full_name, "team repo created");

    // Tag the repo so we can discover it later.
    if let Err(e) = client
        .set_repo_topics(&repo.owner.login, &repo.name, &[TEAM_MARKER_TOPIC])
        .await
    {
        tracing::warn!(error = %e, "failed to set repo topic; team will still work but won't appear in auto-discovery");
    }

    let team_id = Uuid::new_v4().to_string();
    let manifest = TeamManifest {
        schema_version: TEAM_SCHEMA_VERSION,
        team_id: team_id.clone(),
        team_name: name.clone(),
        created_at: now_ms(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).context("serialize team manifest")?;

    // Retry a few times: the GitHub API can take a second or two after repo
    // creation (with auto_init) before the contents endpoint accepts writes.
    // On each retry we re-fetch the file's SHA — if a prior attempt actually
    // succeeded server-side but the response got lost, the next PUT needs the
    // existing SHA or GitHub rejects it with 422 "sha wasn't supplied".
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..6u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
        }

        let prev_sha = client
            .get_file(&repo.owner.login, &repo.name, ".stake-team.json")
            .await
            .ok()
            .flatten()
            .map(|f| f.sha);

        match client
            .put_file(
                &repo.owner.login,
                &repo.name,
                ".stake-team.json",
                &manifest_bytes,
                "chore: initialise team workspace",
                prev_sha.as_deref(),
            )
            .await
        {
            Ok(_) => {
                last_err = None;
                break;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "write team manifest failed, retrying");
                last_err = Some(e);
            }
        }
    }
    if let Some(e) = last_err {
        return Err(e).context("write team manifest after retries");
    }

    // Seed README so the repo landing page makes sense on github.com.
    let readme = format!(
        "# {name}\n\n\
        This repository is a **stake-dev-tool team workspace**.\n\n\
        It synchronises profiles, saved rounds, and (optionally) math files \
        between team members via the stake-dev-tool desktop app.\n\n\
        - App: <https://github.com/simnJS/stake-dev-tool>\n\
        - Schema version: {TEAM_SCHEMA_VERSION}\n\n\
        > Don't edit files in this repo directly — use the app.\n"
    );
    client
        .put_file(
            &repo.owner.login,
            &repo.name,
            "README.md",
            readme.as_bytes(),
            "docs: seed README",
            None,
        )
        .await
        .ok();

    let team = Team {
        id: team_id,
        name,
        repo_owner: repo.owner.login.clone(),
        repo_name: repo.name.clone(),
        role: TeamRole::Owner,
        html_url: repo.html_url.clone(),
        added_at: now_ms(),
        last_sync_at: None,
        migrated_to: None,
    };
    upsert_local(team).await
}

pub async fn join_team(owner: String, name: String) -> Result<Team> {
    let client = GithubClient::from_stored_token()?;
    let repo: RepoInfo = client.get_repo(&owner, &name).await?;
    let manifest_file = client
        .get_file(&repo.owner.login, &repo.name, ".stake-team.json")
        .await?
        .ok_or_else(|| anyhow!("repo is missing .stake-team.json — not a team workspace"))?;
    let manifest: TeamManifest =
        serde_json::from_slice(&manifest_file.content).context("parse team manifest")?;

    if manifest.schema_version > TEAM_SCHEMA_VERSION {
        return Err(anyhow!(
            "team repo uses schema v{} but this app only supports v{}. Please update the app.",
            manifest.schema_version,
            TEAM_SCHEMA_VERSION
        ));
    }

    // Best-effort topic tag (owner only).
    let _ = client
        .set_repo_topics(&repo.owner.login, &repo.name, &[TEAM_MARKER_TOPIC])
        .await;

    // Detect if we're the owner of this repo so the UI shows the right badge
    // and exposes owner-only actions (invite, delete). For personal repos the
    // owner.login matches the authenticated user; for org repos we'd need the
    // collaborator permission — treating those as Member is fine since we
    // don't expose destructive owner-only actions on them anyway.
    let current = crate::github::auth::current_user().await?;
    let role = match &current {
        Some(u) if u.login.eq_ignore_ascii_case(&repo.owner.login) => TeamRole::Owner,
        _ => TeamRole::Member,
    };

    let team = Team {
        id: manifest.team_id,
        name: manifest.team_name,
        repo_owner: repo.owner.login,
        repo_name: repo.name,
        role,
        html_url: repo.html_url,
        added_at: now_ms(),
        last_sync_at: None,
        migrated_to: None,
    };
    upsert_local(team).await
}

pub async fn invite_member(team_id: &str, username: &str) -> Result<()> {
    let team = {
        let f = load().await?;
        f.teams
            .into_iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("team not found"))?
    };
    if team.role != TeamRole::Owner {
        return Err(anyhow!("only the team owner can invite members"));
    }
    let client = GithubClient::from_stored_token()?;
    client
        .invite_collaborator(&team.repo_owner, &team.repo_name, username)
        .await
}

/// Remove a profile from a team's catalogue. Owner-only — deletes the
/// profile JSON + math release + any saved rounds tagged with its gameSlug.
/// Doesn't touch the caller's local copy.
pub async fn remove_from_catalog(team_id: &str, profile_id: &str) -> Result<()> {
    let team = {
        let f = load().await?;
        f.teams
            .into_iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("team not found"))?
    };
    if team.role != TeamRole::Owner {
        return Err(anyhow!(
            "only the team owner can remove a profile from the catalogue"
        ));
    }
    let client = GithubClient::from_stored_token()?;

    // Fetch the profile first so we know its gameSlug (needed to find the
    // release + purge rounds).
    let profile_path = format!("profiles/{profile_id}.json");
    let Some(file) = client
        .get_file(&team.repo_owner, &team.repo_name, &profile_path)
        .await?
    else {
        return Err(anyhow!("profile not found in team catalogue"));
    };
    let remote: crate::profiles::Profile =
        serde_json::from_slice(&file.content).context("parse team profile")?;

    // Delete the release (math files) first — if this fails we don't want to
    // orphan the profile JSON pointing at nothing.
    let tag = format!("math-{}", remote.game_slug);
    if let Some(release) = client
        .find_release_by_tag(&team.repo_owner, &team.repo_name, &tag)
        .await?
    {
        // Delete assets + release. GitHub's release DELETE endpoint removes
        // the tag entry and its assets. Tag itself remains as a git ref but
        // it's harmless.
        for asset in &release.assets {
            client
                .delete_release_asset(&team.repo_owner, &team.repo_name, asset.id)
                .await
                .ok();
        }
        // There's no `delete_release` method on our client yet — expose it
        // via raw request. (Kept inline to keep the surface area small.)
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/{}",
            team.repo_owner, team.repo_name, release.id
        );
        // Don't propagate: a stale release tag won't break re-publish (the
        // engine reuses it). But DO log: silently dropping the failure means
        // a "Removed from team" toast can fire while a ghost release lingers
        // on github.com with no debuggable trail.
        match reqwest::Client::builder()
            .user_agent("stake-dev-tool")
            .build()
            .context("build client")?
            .delete(&url)
            .bearer_auth(crate::github::auth::load_token()?.ok_or_else(|| anyhow!("no token"))?)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
        {
            Ok(res) if !res.status().is_success() => {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                tracing::warn!(
                    %status,
                    body = %body,
                    release_id = release.id,
                    "release DELETE failed; tag may be orphaned"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, release_id = release.id, "release DELETE transport error");
            }
            _ => {}
        }
    }

    // Delete the manifest + profile JSON.
    let manifest_path = format!("math-manifests/{}.json", remote.game_slug);
    if let Ok(Some(m)) = client
        .get_file(&team.repo_owner, &team.repo_name, &manifest_path)
        .await
    {
        client
            .delete_file(
                &team.repo_owner,
                &team.repo_name,
                &manifest_path,
                &m.sha,
                &format!("catalogue: drop math manifest for {}", remote.game_slug),
            )
            .await
            .ok();
    }
    client
        .delete_file(
            &team.repo_owner,
            &team.repo_name,
            &profile_path,
            &file.sha,
            &format!("catalogue: remove profile {}", remote.name),
        )
        .await?;

    // Saved rounds for that game → drop them too. Best-effort.
    let round_entries = client
        .list_dir(&team.repo_owner, &team.repo_name, "saved-rounds")
        .await
        .unwrap_or_default();
    for entry in round_entries {
        if entry.kind != "file" || !entry.name.ends_with(".json") {
            continue;
        }
        let Some(file) = client
            .get_file(&team.repo_owner, &team.repo_name, &entry.path)
            .await
            .ok()
            .flatten()
        else {
            continue;
        };
        let Ok(round) = serde_json::from_slice::<lgs::saved_rounds::SavedRound>(&file.content)
        else {
            continue;
        };
        if round.game_slug == remote.game_slug {
            client
                .delete_file(
                    &team.repo_owner,
                    &team.repo_name,
                    &entry.path,
                    &file.sha,
                    &format!("catalogue: drop round for {}", remote.game_slug),
                )
                .await
                .ok();
        }
    }

    Ok(())
}

/// Upload (or update) a single local profile into the team repo so it
/// appears in the team's catalogue for other members, alongside every saved
/// round that belongs to that game. One call = everything other members
/// need to replicate the profile end-to-end (minus the math, which ships via
/// `math_sync::push`).
pub async fn push_local_profile(team_id: &str, profile_id: &str) -> Result<()> {
    let team = {
        let f = load().await?;
        f.teams
            .into_iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("team not found"))?
    };
    let mut profile = crate::profiles::list()
        .await?
        .into_iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| anyhow!("profile not found"))?;

    let client = GithubClient::from_stored_token()?;

    // Strip machine-specific metadata before uploading. Other members fill in
    // their own gamePath at pull-time; `team_id` is a local-origin marker
    // that shouldn't ship with the shared JSON.
    let profile_name = profile.name.clone();
    profile.game_path = String::new();
    profile.team_id = None;

    let profile_path = format!("profiles/{}.json", profile.id);
    let prev_sha = client
        .get_file(&team.repo_owner, &team.repo_name, &profile_path)
        .await
        .ok()
        .flatten()
        .map(|f| f.sha);
    let bytes = serde_json::to_vec_pretty(&profile).context("serialize profile")?;
    client
        .put_file(
            &team.repo_owner,
            &team.repo_name,
            &profile_path,
            &bytes,
            &format!("catalogue: {}", profile_name),
            prev_sha.as_deref(),
        )
        .await?;

    // Bundle the saved rounds that belong to this profile's game. That way
    // "share my profile" carries over interesting round bookmarks, not just
    // the cosmetic metadata.
    let rounds = lgs::saved_rounds::list(Some(&profile.game_slug)).await?;
    for round in rounds {
        let round_path = format!("saved-rounds/{}.json", round.id);
        let round_prev_sha = client
            .get_file(&team.repo_owner, &team.repo_name, &round_path)
            .await
            .ok()
            .flatten()
            .map(|f| f.sha);
        let round_bytes = serde_json::to_vec_pretty(&round).context("serialize round")?;
        if let Err(e) = client
            .put_file(
                &team.repo_owner,
                &team.repo_name,
                &round_path,
                &round_bytes,
                &format!("round: {} ({})", &round.id[..8], profile.game_slug),
                round_prev_sha.as_deref(),
            )
            .await
        {
            // Non-fatal: the profile itself was pushed; rounds are secondary.
            tracing::warn!(round_id = %round.id, error = %e, "failed to push saved round");
        }
    }

    // Stamp the local profile as belonging to this team so the UI moves it
    // to the team group and exposes the "Pull latest" action.
    crate::profiles::set_team(profile_id, Some(team_id))
        .await
        .ok();

    Ok(())
}

/// Fetch all profiles stored in the team repo alongside their math-manifest
/// availability. Used by the main page to render the team's game catalogue
/// even before the local user has pulled anything.
pub async fn list_team_profiles(team_id: &str) -> Result<Vec<TeamProfileInfo>> {
    let team = {
        let f = load().await?;
        f.teams
            .into_iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("team not found"))?
    };
    let client = GithubClient::from_stored_token()?;

    let profile_entries = client
        .list_dir(&team.repo_owner, &team.repo_name, "profiles")
        .await?;
    let manifest_entries = client
        .list_dir(&team.repo_owner, &team.repo_name, "math-manifests")
        .await?;
    let available_math: std::collections::HashSet<String> = manifest_entries
        .into_iter()
        .filter(|e| e.kind == "file" && e.name.ends_with(".json"))
        .map(|e| e.name.trim_end_matches(".json").to_string())
        .collect();

    let mut out = Vec::new();
    for entry in profile_entries {
        if entry.kind != "file" || !entry.name.ends_with(".json") {
            continue;
        }
        let Some(file) = client
            .get_file(&team.repo_owner, &team.repo_name, &entry.path)
            .await?
        else {
            continue;
        };
        let Ok(p) = serde_json::from_slice::<crate::profiles::Profile>(&file.content) else {
            tracing::warn!(path = %entry.path, "skip malformed team profile");
            continue;
        };
        let has_math = available_math.contains(&p.game_slug);
        out.push(TeamProfileInfo {
            id: p.id,
            name: p.name,
            game_slug: p.game_slug,
            game_url: p.game_url,
            has_math,
            updated_at: p.updated_at,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Pull math for a team profile and create/update a local profile pointing at
/// the pulled folder. Composes `math_sync::pull` + `profiles::upsert` so the
/// main-page "Pull" button is one-click.
pub async fn pull_team_profile(
    app: &tauri::AppHandle,
    team_id: &str,
    team_profile_id: &str,
) -> Result<crate::profiles::Profile> {
    let team = {
        let f = load().await?;
        f.teams
            .into_iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("team not found"))?
    };
    let client = GithubClient::from_stored_token()?;

    // Fetch the team profile metadata (name, gameSlug, gameUrl, resolutions).
    let path = format!("profiles/{team_profile_id}.json");
    let file = client
        .get_file(&team.repo_owner, &team.repo_name, &path)
        .await?
        .ok_or_else(|| anyhow!("team profile not found"))?;
    let remote: crate::profiles::Profile =
        serde_json::from_slice(&file.content).context("parse team profile")?;

    // Download math into the default team math root / <gameSlug>/.
    let root = default_team_math_root(&team)?;
    let game_dest = root.join(&remote.game_slug);
    tokio::fs::create_dir_all(&game_dest).await.ok();
    crate::math_sync::pull(
        app,
        team_id,
        &remote.game_slug,
        game_dest.to_string_lossy().into_owned(),
    )
    .await
    .context("pull math for team profile")?;

    // Upsert a local profile reusing the team profile's id so subsequent syncs
    // stay aligned. We stamp `team_id` so the UI can group/filter by origin.
    let local = crate::profiles::upsert_raw(crate::profiles::Profile {
        id: remote.id,
        name: remote.name,
        game_path: game_dest.to_string_lossy().into_owned(),
        game_url: remote.game_url,
        game_slug: remote.game_slug.clone(),
        resolutions: remote.resolutions,
        created_at: remote.created_at,
        updated_at: now_ms(),
        team_id: Some(team_id.to_string()),
    })
    .await?;

    // Pull saved rounds that belong to this game — best-effort, don't fail
    // the whole pull over a missing round.
    if let Err(e) = pull_rounds_for_game(&client, &team, &local.game_slug).await {
        tracing::warn!(error = %e, "failed to pull saved rounds for {}", local.game_slug);
    }

    Ok(local)
}

async fn pull_rounds_for_game(client: &GithubClient, team: &Team, game_slug: &str) -> Result<()> {
    let entries = client
        .list_dir(&team.repo_owner, &team.repo_name, "saved-rounds")
        .await?;
    for entry in entries {
        if entry.kind != "file" || !entry.name.ends_with(".json") {
            continue;
        }
        let Some(file) = client
            .get_file(&team.repo_owner, &team.repo_name, &entry.path)
            .await?
        else {
            continue;
        };
        let Ok(round) = serde_json::from_slice::<lgs::saved_rounds::SavedRound>(&file.content)
        else {
            continue;
        };
        if round.game_slug != game_slug {
            continue;
        }
        lgs::saved_rounds::upsert_raw(round).await.ok();
    }
    Ok(())
}

/// Flat catalogue across every team the user is a member of, with the source
/// team's id + name stamped on each entry. Lets the main page render multiple
/// teams' shared games without caring which one is "active".
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    #[serde(rename = "teamId")]
    pub team_id: String,
    #[serde(rename = "teamName")]
    pub team_name: String,
    pub profile: TeamProfileInfo,
}

pub async fn list_all_catalogs() -> Result<Vec<CatalogEntry>> {
    let teams = list_local().await?;
    let mut out = Vec::new();
    for t in &teams {
        // Each team call hits the GitHub API serially. For small N (~1-5
        // teams) this is fine; if it becomes an issue we can parallelise
        // with `tokio::try_join_all`.
        match list_team_profiles(&t.id).await {
            Ok(profiles) => {
                for p in profiles {
                    out.push(CatalogEntry {
                        team_id: t.id.clone(),
                        team_name: t.name.clone(),
                        profile: p,
                    });
                }
            }
            Err(e) => {
                tracing::warn!(team = %t.name, error = %e, "failed to list team catalogue");
            }
        }
    }

    // Reconcile: profiles pushed before the team_id field existed (or before
    // set_team was wired into push) still carry `team_id: None` locally even
    // though their ID appears in the team's catalogue. Stamp them so the UI
    // moves them to the right group without requiring a manual re-push.
    if let Ok(locals) = crate::profiles::list().await {
        for local in locals {
            if local.team_id.is_some() {
                continue;
            }
            if let Some(entry) = out.iter().find(|c| c.profile.id == local.id) {
                crate::profiles::set_team(&local.id, Some(&entry.team_id))
                    .await
                    .ok();
            }
        }
    }

    Ok(out)
}

pub async fn discover_teams() -> Result<Vec<DiscoveredTeam>> {
    let client = GithubClient::from_stored_token()?;
    let repos = client.list_team_repos(TEAM_MARKER_TOPIC).await?;
    let mut out = Vec::with_capacity(repos.len());
    for repo in repos {
        let Ok(Some(manifest_file)) = client
            .get_file(&repo.owner.login, &repo.name, ".stake-team.json")
            .await
        else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<TeamManifest>(&manifest_file.content) else {
            continue;
        };
        out.push(DiscoveredTeam {
            team_id: manifest.team_id,
            team_name: manifest.team_name,
            repo_owner: repo.owner.login,
            repo_name: repo.name,
            html_url: repo.html_url,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredTeam {
    #[serde(rename = "teamId")]
    pub team_id: String,
    #[serde(rename = "teamName")]
    pub team_name: String,
    #[serde(rename = "repoOwner")]
    pub repo_owner: String,
    #[serde(rename = "repoName")]
    pub repo_name: String,
    #[serde(rename = "htmlUrl")]
    pub html_url: String,
}

// ============================================================
// Sync: profiles + saved rounds
// ============================================================

pub async fn sync_team(team_id: &str) -> Result<SyncReport> {
    let team = {
        let f = load().await?;
        f.teams
            .into_iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("team not found"))?
    };
    let client = GithubClient::from_stored_token()?;

    // Profiles are NOT bidi-synced here: other members' profiles reference
    // machine-specific local paths, so applying them blindly produces broken
    // "launchable" entries pointing at someone else's disk. Profiles are now
    // browsed via the team catalogue (`list_team_profiles`) and fetched
    // explicitly with `pull_team_profile`, which writes a correct local path.
    let (rounds_pushed, rounds_pulled) = sync_saved_rounds(&client, &team).await?;

    let mut f = load().await?;
    if let Some(t) = f.teams.iter_mut().find(|t| t.id == team_id) {
        t.last_sync_at = Some(now_ms());
    }
    save(&f).await?;

    Ok(SyncReport {
        profiles_pushed: 0,
        profiles_pulled: 0,
        rounds_pushed,
        rounds_pulled,
    })
}

#[allow(dead_code)]
async fn sync_profiles(client: &GithubClient, team: &Team) -> Result<(u32, u32)> {
    let local = profiles::list().await?;
    let remote_entries = client
        .list_dir(&team.repo_owner, &team.repo_name, "profiles")
        .await?;

    let mut pushed = 0u32;
    let mut pulled = 0u32;

    let mut remote_by_id: std::collections::HashMap<String, crate::github::api::RepoEntry> =
        std::collections::HashMap::new();
    for e in remote_entries {
        if e.kind != "file" || !e.name.ends_with(".json") {
            continue;
        }
        let id = e.name.trim_end_matches(".json").to_string();
        remote_by_id.insert(id, e);
    }

    // Push local → remote (newer updatedAt wins).
    for p in &local {
        let bytes = serde_json::to_vec_pretty(p).context("serialize profile")?;
        let path = format!("profiles/{}.json", p.id);

        let prev_sha = remote_by_id.get(&p.id).map(|e| e.sha.clone());
        let should_write = match &prev_sha {
            None => true,
            Some(_) => {
                // Fetch and compare to decide if local is newer.
                match client
                    .get_file(&team.repo_owner, &team.repo_name, &path)
                    .await?
                {
                    Some(f) => match serde_json::from_slice::<Profile>(&f.content) {
                        Ok(remote_p) => p.updated_at > remote_p.updated_at,
                        Err(_) => true,
                    },
                    None => true,
                }
            }
        };

        if should_write {
            client
                .put_file(
                    &team.repo_owner,
                    &team.repo_name,
                    &path,
                    &bytes,
                    &format!("sync: profile {}", p.name),
                    prev_sha.as_deref(),
                )
                .await?;
            pushed += 1;
        }
    }

    // Pull remote → local (anything that's on remote but not local or
    // remote is newer).
    let local_ids: std::collections::HashSet<String> = local.iter().map(|p| p.id.clone()).collect();
    for (id, entry) in &remote_by_id {
        let path = &entry.path;
        let Some(file) = client
            .get_file(&team.repo_owner, &team.repo_name, path)
            .await?
        else {
            continue;
        };
        let remote_p: Profile = match serde_json::from_slice(&file.content) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, path = %path, "skip malformed profile");
                continue;
            }
        };

        if !local_ids.contains(id) {
            profiles::upsert_raw(remote_p).await?;
            pulled += 1;
        } else {
            let local_p = local.iter().find(|lp| lp.id == *id).unwrap();
            if remote_p.updated_at > local_p.updated_at {
                profiles::upsert_raw(remote_p).await?;
                pulled += 1;
            }
        }
    }

    Ok((pushed, pulled))
}

async fn sync_saved_rounds(client: &GithubClient, team: &Team) -> Result<(u32, u32)> {
    // Resolve the games this team actually catalogues from its `profiles/`
    // directory. Without this filter, `lgs::saved_rounds::list(None)` would
    // return every round on disk (including rounds for purely-local profiles
    // and rounds for OTHER teams' games), and we'd happily push all of them
    // into this team's repo — leaking them to every member on next sync.
    // Mirrors the per-game filter `push_local_profile` already uses.
    let team_profile_entries = client
        .list_dir(&team.repo_owner, &team.repo_name, "profiles")
        .await
        .unwrap_or_default();
    let mut team_slugs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in team_profile_entries {
        if e.kind != "file" || !e.name.ends_with(".json") {
            continue;
        }
        if let Ok(Some(file)) = client
            .get_file(&team.repo_owner, &team.repo_name, &e.path)
            .await
            && let Ok(p) = serde_json::from_slice::<crate::profiles::Profile>(&file.content)
        {
            team_slugs.insert(p.game_slug);
        }
    }
    let local: Vec<lgs::saved_rounds::SavedRound> = lgs::saved_rounds::list(None)
        .await?
        .into_iter()
        .filter(|r| team_slugs.contains(&r.game_slug))
        .collect();
    let remote_entries = client
        .list_dir(&team.repo_owner, &team.repo_name, "saved-rounds")
        .await?;

    let mut pushed = 0u32;
    let mut pulled = 0u32;

    let mut remote_by_id: std::collections::HashMap<String, crate::github::api::RepoEntry> =
        std::collections::HashMap::new();
    for e in remote_entries {
        if e.kind != "file" || !e.name.ends_with(".json") {
            continue;
        }
        let id = e.name.trim_end_matches(".json").to_string();
        remote_by_id.insert(id, e);
    }

    for r in &local {
        let bytes = serde_json::to_vec_pretty(r).context("serialize round")?;
        let path = format!("saved-rounds/{}.json", r.id);

        let prev_sha = remote_by_id.get(&r.id).map(|e| e.sha.clone());
        let should_write = match &prev_sha {
            None => true,
            Some(_) => match client
                .get_file(&team.repo_owner, &team.repo_name, &path)
                .await?
            {
                Some(f) => {
                    match serde_json::from_slice::<lgs::saved_rounds::SavedRound>(&f.content) {
                        Ok(remote_r) => r.updated_at > remote_r.updated_at,
                        Err(_) => true,
                    }
                }
                None => true,
            },
        };

        if should_write {
            client
                .put_file(
                    &team.repo_owner,
                    &team.repo_name,
                    &path,
                    &bytes,
                    &format!("sync: round {}", &r.id[..8]),
                    prev_sha.as_deref(),
                )
                .await?;
            pushed += 1;
        }
    }

    let local_ids: std::collections::HashSet<String> = local.iter().map(|r| r.id.clone()).collect();
    for (id, entry) in &remote_by_id {
        let Some(file) = client
            .get_file(&team.repo_owner, &team.repo_name, &entry.path)
            .await?
        else {
            continue;
        };
        let remote_r: lgs::saved_rounds::SavedRound = match serde_json::from_slice(&file.content) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "skip malformed saved round");
                continue;
            }
        };
        // Defensive symmetry with the push filter above: if the team repo
        // contains a round whose game isn't catalogued by this team (legacy
        // data from before the push filter, or a manual push outside the
        // app), don't pull it into the local DB. Otherwise the user accumulates
        // orphan bookmarks for games they may not have pulled.
        if !team_slugs.contains(&remote_r.game_slug) {
            continue;
        }
        if !local_ids.contains(id) {
            lgs::saved_rounds::upsert_raw(remote_r).await?;
            pulled += 1;
        } else {
            let local_r = local.iter().find(|lr| lr.id == *id).unwrap();
            if remote_r.updated_at > local_r.updated_at {
                lgs::saved_rounds::upsert_raw(remote_r).await?;
                pulled += 1;
            }
        }
    }

    Ok((pushed, pulled))
}
