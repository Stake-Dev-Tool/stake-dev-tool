//! Document sync + catalogue orchestration over [`super::documents`],
//! [`super::math`], [`super::sidecar`] and [`super::workspaces`].
//!
//! - [`sync`] backs `teams_sync`: pull newer documents (`?since_seq=`), then
//!   push local profiles + saved rounds with client-driven last-write-wins
//!   (`document_conflict` → adopt server revision, retry keeping ours).
//! - The `*_catalog` / `push_profile` / `pull_profile` helpers back the
//!   launcher's per-workspace share/pull flow (re-pointed from the GitHub
//!   catalogue to cloud documents + M2 math).
//!
//! `gamePath` never travels: profiles are split/merged through
//! [`super::sidecar`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tauri::AppHandle;

use super::documents::{
    CloudDocuments, DocError, DocumentApi, DocumentEnvelope, KIND_PROFILE, KIND_SAVED_ROUND,
    ProfileDoc, SavedRoundDoc, put_lww,
};
use super::{math, sidecar, workspaces};

/// Result of a `teams_sync`, surfaced to the UI.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncReport {
    pub pushed: u32,
    pub pulled: u32,
    pub conflicts: u32,
}

/// One shared profile in a workspace catalogue (UI shape unchanged from V1).
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

/// A catalogue entry stamped with its workspace (UI shape unchanged from V1).
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    #[serde(rename = "teamId")]
    pub team_id: String,
    #[serde(rename = "teamName")]
    pub team_name: String,
    pub profile: TeamProfileInfo,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// RFC3339 → epoch ms, best-effort (0 when absent/unparseable).
fn parse_ts_ms(ts: &Option<String>) -> u64 {
    ts.as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis().max(0) as u64)
        .unwrap_or(0)
}

/// Default on-disk location for math pulled from a workspace:
/// `<documents>/stake-dev-tool/workspaces/<slug>/`.
pub fn default_math_root(slug: &str) -> Result<PathBuf> {
    let docs = dirs::document_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow!("could not resolve documents directory"))?;
    Ok(docs.join("stake-dev-tool").join("workspaces").join(slug))
}

// ---------------------------------------------------------------------------
// Full sync
// ---------------------------------------------------------------------------

/// Pull-then-push document sync for one workspace. See the module docs.
pub async fn sync(workspace_id: &str) -> Result<SyncReport> {
    let entry = workspaces::resolve(workspace_id).await?;
    let docs = CloudDocuments::new(&entry.slug).map_err(|e| anyhow!("{e}"))?;
    let mut report = SyncReport::default();

    // ---- Pull ----
    let since = workspaces::last_seq(workspace_id).await.unwrap_or(0);
    let list = docs
        .list(None, Some(since))
        .await
        .map_err(|e| anyhow!("{e}"))?;

    // Remember server revisions so the push below sends the right base_revision.
    let mut server_rev: HashMap<(String, String), i64> = HashMap::new();
    for env in &list.documents {
        server_rev.insert((env.kind.clone(), env.doc_id.clone()), env.revision);
        apply_pulled(workspace_id, env, &mut report).await;
    }
    if list.latest_seq > since {
        workspaces::set_last_seq(workspace_id, list.latest_seq)
            .await
            .ok();
    }

    // ---- Push ----
    // Profiles belonging to this workspace, plus the saved rounds of their games.
    let locals = crate::profiles::list().await.unwrap_or_default();
    let mine: Vec<_> = locals
        .into_iter()
        .filter(|p| p.team_id.as_deref() == Some(workspace_id))
        .collect();
    let my_slugs: std::collections::HashSet<String> =
        mine.iter().map(|p| p.game_slug.clone()).collect();

    for p in &mine {
        let doc = sidecar::profile_to_doc(p);
        let data = serde_json::to_value(&doc).context("serialize profile doc")?;
        let base = server_rev
            .get(&(KIND_PROFILE.to_string(), p.id.clone()))
            .copied();
        match put_lww(&docs, KIND_PROFILE, &p.id, &data, base).await {
            Ok(_) => report.pushed += 1,
            Err(DocError::Conflict { .. }) => report.conflicts += 1,
            Err(e) => return Err(anyhow!("push profile {}: {e}", p.name)),
        }
    }

    let rounds = lgs::saved_rounds::list(None).await.unwrap_or_default();
    for r in rounds.iter().filter(|r| my_slugs.contains(&r.game_slug)) {
        let doc = SavedRoundDoc {
            game_slug: r.game_slug.clone(),
            mode: r.mode.clone(),
            event_id: r.event_id,
            description: r.description.clone(),
            revision: None,
            created_at: r.created_at,
        };
        let data = serde_json::to_value(&doc).context("serialize round doc")?;
        let base = server_rev
            .get(&(KIND_SAVED_ROUND.to_string(), r.id.clone()))
            .copied();
        match put_lww(&docs, KIND_SAVED_ROUND, &r.id, &data, base).await {
            Ok(_) => report.pushed += 1,
            Err(DocError::Conflict { .. }) => report.conflicts += 1,
            Err(e) => return Err(anyhow!("push round {}: {e}", r.id)),
        }
    }

    Ok(report)
}

/// Applies one pulled document to the local stores (LWW: server state wins for
/// pulled changes; the local `gamePath` overlay is preserved).
async fn apply_pulled(workspace_id: &str, env: &DocumentEnvelope, report: &mut SyncReport) {
    match env.kind.as_str() {
        KIND_PROFILE => {
            if env.deleted {
                crate::profiles::delete(&env.doc_id).await.ok();
                report.pulled += 1;
                return;
            }
            let Ok(doc) = serde_json::from_value::<ProfileDoc>(env.data.clone()) else {
                tracing::warn!(doc_id = %env.doc_id, "skip malformed profile document");
                return;
            };
            let game_path = sidecar::local_game_path(&env.doc_id).await;
            // Local mtime = pull time so the launcher's "update available" check
            // (remote updatedAt > local) only fires when the server changes next.
            let profile =
                sidecar::doc_to_profile(workspace_id, &env.doc_id, doc, game_path, now_ms());
            if crate::profiles::upsert_raw(profile).await.is_ok() {
                report.pulled += 1;
            }
        }
        KIND_SAVED_ROUND => {
            if env.deleted {
                lgs::saved_rounds::delete(&env.doc_id).await.ok();
                report.pulled += 1;
                return;
            }
            let Ok(doc) = serde_json::from_value::<SavedRoundDoc>(env.data.clone()) else {
                tracing::warn!(doc_id = %env.doc_id, "skip malformed saved_round document");
                return;
            };
            let round = lgs::saved_rounds::SavedRound {
                id: env.doc_id.clone(),
                game_slug: doc.game_slug,
                mode: doc.mode,
                event_id: doc.event_id,
                description: doc.description,
                created_at: doc.created_at,
                updated_at: now_ms(),
            };
            if lgs::saved_rounds::upsert_raw(round).await.is_ok() {
                report.pulled += 1;
            }
        }
        other => tracing::debug!(kind = %other, "ignoring unknown document kind on pull"),
    }
}

// ---------------------------------------------------------------------------
// Catalogue (launcher share/pull)
// ---------------------------------------------------------------------------

/// Every profile document across every workspace, shaped for the launcher's
/// catalogue. `hasMath` reflects whether the workspace has a math revision for
/// the profile's game.
pub async fn all_catalogs() -> Result<Vec<CatalogEntry>> {
    let client = super::api::CloudClient::from_stored_token().map_err(|e| anyhow!("{e}"))?;
    let wss = workspaces::list().await?;
    let mut out = Vec::new();
    for ws in &wss {
        let docs = match CloudDocuments::new(&ws.slug) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let games_with_math: std::collections::HashSet<String> = client
            .list_games(&ws.slug)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|g| g.head_number.is_some())
            .map(|g| g.slug)
            .collect();
        let Ok(list) = docs.list(Some(KIND_PROFILE), None).await else {
            continue;
        };
        for env in list.documents {
            if env.deleted {
                continue;
            }
            let Ok(doc) = serde_json::from_value::<ProfileDoc>(env.data) else {
                continue;
            };
            out.push(CatalogEntry {
                team_id: ws.id.clone(),
                team_name: ws.name.clone(),
                profile: TeamProfileInfo {
                    id: env.doc_id,
                    name: doc.name,
                    has_math: games_with_math.contains(&doc.game_slug),
                    game_slug: doc.game_slug,
                    game_url: doc.front_url.unwrap_or_default(),
                    updated_at: parse_ts_ms(&env.updated_at),
                },
            });
        }
    }
    out.sort_by(|a, b| a.profile.name.cmp(&b.profile.name));
    Ok(out)
}

/// Lists one workspace's shared profiles.
pub async fn list_profiles(workspace_id: &str) -> Result<Vec<TeamProfileInfo>> {
    Ok(all_catalogs()
        .await?
        .into_iter()
        .filter(|c| c.team_id == workspace_id)
        .map(|c| c.profile)
        .collect())
}

/// Shares a local profile (+ its saved rounds) into a workspace as documents.
/// Math is pushed separately via [`push_math`].
pub async fn push_profile(workspace_id: &str, profile_id: &str) -> Result<()> {
    let entry = workspaces::resolve(workspace_id).await?;
    let docs = CloudDocuments::new(&entry.slug).map_err(|e| anyhow!("{e}"))?;

    let profile = crate::profiles::list()
        .await?
        .into_iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| anyhow!("profile not found"))?;

    let doc = sidecar::profile_to_doc(&profile);
    let data = serde_json::to_value(&doc)?;
    let base = docs
        .get(KIND_PROFILE, profile_id)
        .await
        .ok()
        .flatten()
        .map(|e| e.revision);
    put_lww(&docs, KIND_PROFILE, profile_id, &data, base)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    // Bundle the saved rounds for this profile's game.
    for r in lgs::saved_rounds::list(Some(&profile.game_slug))
        .await
        .unwrap_or_default()
    {
        let round_doc = SavedRoundDoc {
            game_slug: r.game_slug.clone(),
            mode: r.mode.clone(),
            event_id: r.event_id,
            description: r.description.clone(),
            revision: None,
            created_at: r.created_at,
        };
        let Ok(rd) = serde_json::to_value(&round_doc) else {
            continue;
        };
        let base = docs
            .get(KIND_SAVED_ROUND, &r.id)
            .await
            .ok()
            .flatten()
            .map(|e| e.revision);
        if let Err(e) = put_lww(&docs, KIND_SAVED_ROUND, &r.id, &rd, base).await {
            tracing::warn!(round = %r.id, error = %e, "failed to push saved round");
        }
    }

    // Stamp the local profile as belonging to this workspace.
    crate::profiles::set_team(profile_id, Some(workspace_id))
        .await
        .ok();
    Ok(())
}

/// Pulls a shared profile: fetch the document, pull its math into the default
/// workspace root, write a local profile pointing at it, and pull its rounds.
pub async fn pull_profile(
    app: &AppHandle,
    workspace_id: &str,
    profile_id: &str,
) -> Result<crate::profiles::Profile> {
    let entry = workspaces::resolve(workspace_id).await?;
    let docs = CloudDocuments::new(&entry.slug).map_err(|e| anyhow!("{e}"))?;

    let env = docs
        .get(KIND_PROFILE, profile_id)
        .await
        .map_err(|e| anyhow!("{e}"))?
        .ok_or_else(|| anyhow!("profile not found in workspace"))?;
    let doc: ProfileDoc = serde_json::from_value(env.data).context("parse profile document")?;
    let game_slug = doc.game_slug.clone();

    // Download math (pinned revision from the doc, else latest) into the root.
    let dest = default_math_root(&entry.slug)?.join(&game_slug);
    tokio::fs::create_dir_all(&dest).await.ok();
    math::pull(
        app,
        &entry.slug,
        &game_slug,
        doc.revision,
        dest.to_string_lossy().into_owned(),
    )
    .await
    .map_err(|e| anyhow!("pull math: {e}"))?;

    let profile = sidecar::doc_to_profile(
        workspace_id,
        profile_id,
        doc,
        Some(dest.to_string_lossy().into_owned()),
        now_ms(),
    );
    let saved = crate::profiles::upsert_raw(profile).await?;

    // Pull the game's saved rounds — best-effort.
    if let Ok(list) = docs.list(Some(KIND_SAVED_ROUND), None).await {
        for renv in list.documents {
            if renv.deleted {
                continue;
            }
            let Ok(rd) = serde_json::from_value::<SavedRoundDoc>(renv.data) else {
                continue;
            };
            if rd.game_slug != game_slug {
                continue;
            }
            let round = lgs::saved_rounds::SavedRound {
                id: renv.doc_id,
                game_slug: rd.game_slug,
                mode: rd.mode,
                event_id: rd.event_id,
                description: rd.description,
                created_at: rd.created_at,
                updated_at: now_ms(),
            };
            lgs::saved_rounds::upsert_raw(round).await.ok();
        }
    }

    Ok(saved)
}

/// Removes a shared profile (its document + its game's saved-round documents)
/// from a workspace. Does not touch the caller's local copy.
pub async fn remove_from_catalog(workspace_id: &str, profile_id: &str) -> Result<()> {
    let entry = workspaces::resolve(workspace_id).await?;
    let docs = CloudDocuments::new(&entry.slug).map_err(|e| anyhow!("{e}"))?;

    let existing = docs
        .get(KIND_PROFILE, profile_id)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let Some(env) = existing else {
        return Ok(());
    };
    let game_slug = serde_json::from_value::<ProfileDoc>(env.data.clone())
        .map(|d| d.game_slug)
        .ok();
    docs.delete(KIND_PROFILE, profile_id, Some(env.revision))
        .await
        .map_err(|e| anyhow!("{e}"))?;

    // Drop that game's saved rounds too (best-effort).
    if let Some(slug) = game_slug
        && let Ok(list) = docs.list(Some(KIND_SAVED_ROUND), None).await
    {
        for renv in list.documents {
            if renv.deleted {
                continue;
            }
            let same = serde_json::from_value::<SavedRoundDoc>(renv.data)
                .map(|d| d.game_slug == slug)
                .unwrap_or(false);
            if same {
                docs.delete(KIND_SAVED_ROUND, &renv.doc_id, Some(renv.revision))
                    .await
                    .ok();
            }
        }
    }
    Ok(())
}

/// Games in a workspace that have at least one math revision.
pub async fn list_remote_games(workspace_id: &str) -> Result<Vec<String>> {
    let entry = workspaces::resolve(workspace_id).await?;
    let client = super::api::CloudClient::from_stored_token().map_err(|e| anyhow!("{e}"))?;
    let mut games: Vec<String> = client
        .list_games(&entry.slug)
        .await
        .map_err(|e| anyhow!("{e}"))?
        .into_iter()
        .filter(|g| g.head_number.is_some())
        .map(|g| g.slug)
        .collect();
    games.sort();
    Ok(games)
}

/// Push math for a workspace game as a new revision (auto-parent).
pub async fn push_math(
    app: &AppHandle,
    workspace_id: &str,
    game_slug: &str,
    game_path: String,
) -> Result<math::MathSyncReport> {
    let entry = workspaces::resolve(workspace_id).await?;
    math::push(
        app,
        &entry.slug,
        game_slug,
        game_path,
        format!("desktop push: {game_slug}"),
    )
    .await
    .map_err(|e| anyhow!("{e}"))
}

/// Pull math for a workspace game (latest revision) into `dest_path`.
pub async fn pull_math(
    app: &AppHandle,
    workspace_id: &str,
    game_slug: &str,
    dest_path: String,
) -> Result<math::MathSyncReport> {
    let entry = workspaces::resolve(workspace_id).await?;
    math::pull(app, &entry.slug, game_slug, None, dest_path)
        .await
        .map_err(|e| anyhow!("{e}"))
}

// ---------------------------------------------------------------------------
// Cloud browser: pull a specific revision → local profile
// ---------------------------------------------------------------------------

/// App-managed on-disk location for a *pinned* revision pulled via the desktop
/// cloud browser: `<data_local>/stake-dev-tool/cloud-math/<slug>/<game>/rev<N>/`.
///
/// Deliberately separate from [`default_math_root`] (which mirrors a game's
/// *latest* revision under the user's Documents for the launcher share/pull
/// flow) so pinning revision N never collides with — or is overwritten by — a
/// later "pull latest".
pub fn cloud_math_root(slug: &str, game_slug: &str, number: i64) -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("could not resolve local data dir"))?
        .join("stake-dev-tool")
        .join("cloud-math")
        .join(slug)
        .join(game_slug)
        .join(format!("rev{number}"));
    Ok(dir)
}

/// The killer feature: pull one specific revision's math into the app-managed
/// cloud-math dir and create-or-update a local [`Profile`](crate::profiles::Profile)
/// pointing at it. The download reuses [`math::pull`], so it emits the usual
/// `math-sync-progress` events (the overlay shows progress).
///
/// The profile is local-only (`team_id = None`, so it lands in the launcher's
/// "Mine" group), its `game_slug` is the workspace game slug, its front URL is
/// left blank for the user to fill, and its name defaults to
/// `"<game> rev N (cloud)"` — the suffix the launcher reads to show a
/// "cloud rev N" badge. Re-pulling the same revision de-dupes on the dest path,
/// updating the existing profile instead of piling up copies.
pub async fn pull_revision_to_profile(
    app: &AppHandle,
    workspace_id: &str,
    game_slug: &str,
    number: i64,
    profile_name: Option<String>,
) -> Result<crate::profiles::Profile> {
    let entry = workspaces::resolve(workspace_id).await?;
    let dest = cloud_math_root(&entry.slug, game_slug, number)?;
    tokio::fs::create_dir_all(&dest).await.ok();
    let dest_str = dest.to_string_lossy().into_owned();

    math::pull(app, &entry.slug, game_slug, Some(number), dest_str.clone())
        .await
        .map_err(|e| anyhow!("pull math: {e}"))?;

    let name = profile_name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{game_slug} rev {number} (cloud)"));

    // Create-or-update: dedupe on the dest path so re-pulling the same revision
    // refreshes the existing profile rather than stacking duplicates.
    let existing_id = crate::profiles::list()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.game_path == dest_str)
        .map(|p| p.id);

    let profile = crate::profiles::upsert(
        existing_id,
        name,
        dest_str,
        String::new(), // front URL: left for the user to fill in
        game_slug.to_string(),
        Vec::new(),
    )
    .await?;
    Ok(profile)
}
