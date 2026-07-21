//! One-shot migration of a legacy GitHub-repo "team" into a cloud workspace.
//!
//! Reads the team through the **existing** [`GithubClient`] (manifest, profile
//! JSONs, saved-round JSONs, and math via [`crate::math_sync::pull`] into a temp
//! dir), creates a workspace, PUTs the documents (stripping `gamePath` through
//! [`sidecar`]), pushes the math as the workspace's rev 1 via
//! [`super::math`], and stamps the local team `migratedTo`.
//!
//! Requires both GitHub and cloud auth; a clear error is returned when either is
//! missing.

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tauri::AppHandle;

use crate::github::api::GithubClient;
use crate::profiles::Profile;

use super::documents::{
    CloudDocuments, KIND_PROFILE, KIND_SAVED_ROUND, ProfileDoc, SavedRoundDoc, put_lww,
};
use super::{math, sidecar, workspaces};

/// What a migration produced.
#[derive(Debug, Clone, Serialize)]
pub struct MigrateReport {
    #[serde(rename = "workspaceId")]
    pub workspace_id: String,
    #[serde(rename = "workspaceSlug")]
    pub workspace_slug: String,
    #[serde(rename = "workspaceName")]
    pub workspace_name: String,
    pub profiles: u32,
    pub rounds: u32,
    pub games: u32,
}

/// Legacy GitHub profile JSON → cloud profile payload. `gamePath`/`teamId` are
/// dropped by [`sidecar::profile_to_doc`].
fn profile_to_doc(profile: &Profile) -> ProfileDoc {
    sidecar::profile_to_doc(profile)
}

/// Legacy GitHub saved round → cloud saved-round payload. Legacy imports are
/// left unpinned (`revision = None` = "latest at the time").
fn round_to_doc(round: &lgs::saved_rounds::SavedRound) -> SavedRoundDoc {
    SavedRoundDoc {
        game_slug: round.game_slug.clone(),
        mode: round.mode.clone(),
        event_id: round.event_id,
        description: round.description.clone(),
        revision: None,
        created_at: round.created_at,
    }
}

/// Migrates the GitHub team `team_id` into a freshly-created cloud workspace.
pub async fn migrate_to_cloud(app: &AppHandle, team_id: &str) -> Result<MigrateReport> {
    // Both credentials are required, up front, with actionable errors.
    let gh = GithubClient::from_stored_token()
        .context("GitHub sign-in required to read the team repo")?;
    if super::auth::current_user().await.ok().flatten().is_none() {
        return Err(anyhow!(
            "cloud sign-in required — sign in to the cloud before migrating"
        ));
    }

    let team = crate::teams::team_by_id(team_id)
        .await
        .context("legacy team not found")?;

    // Create the destination workspace (owner = caller) and activate it.
    let ws = workspaces::create(&team.name, None)
        .await
        .context("create workspace for migration")?;
    let docs = CloudDocuments::new(&ws.slug).map_err(|e| anyhow!("{e}"))?;

    // ---- Profiles ----
    let mut profiles = 0u32;
    let profile_entries = gh
        .list_dir(&team.repo_owner, &team.repo_name, "profiles")
        .await
        .unwrap_or_default();
    for entry in profile_entries {
        if entry.kind != "file" || !entry.name.ends_with(".json") {
            continue;
        }
        let Some(file) = gh
            .get_file(&team.repo_owner, &team.repo_name, &entry.path)
            .await
            .ok()
            .flatten()
        else {
            continue;
        };
        let Ok(profile) = serde_json::from_slice::<Profile>(&file.content) else {
            tracing::warn!(path = %entry.path, "skip malformed team profile during migration");
            continue;
        };
        let doc = profile_to_doc(&profile);
        let data = serde_json::to_value(&doc)?;
        if put_lww(&docs, KIND_PROFILE, &profile.id, &data, None)
            .await
            .is_ok()
        {
            profiles += 1;
        }
    }

    // ---- Saved rounds ----
    let mut rounds = 0u32;
    let round_entries = gh
        .list_dir(&team.repo_owner, &team.repo_name, "saved-rounds")
        .await
        .unwrap_or_default();
    for entry in round_entries {
        if entry.kind != "file" || !entry.name.ends_with(".json") {
            continue;
        }
        let Some(file) = gh
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
        let doc = round_to_doc(&round);
        let data = serde_json::to_value(&doc)?;
        if put_lww(&docs, KIND_SAVED_ROUND, &round.id, &data, None)
            .await
            .is_ok()
        {
            rounds += 1;
        }
    }

    // ---- Math (GitHub Release → temp dir → cloud rev 1) ----
    let mut games = 0u32;
    let game_slugs = crate::math_sync::list_remote_games(team_id)
        .await
        .unwrap_or_default();
    if !game_slugs.is_empty() {
        let tmp = tempfile::tempdir().context("create temp dir for math migration")?;
        for slug in game_slugs {
            let dest = tmp.path().join(&slug);
            if let Err(e) =
                crate::math_sync::pull(app, team_id, &slug, dest.to_string_lossy().into_owned())
                    .await
            {
                tracing::warn!(game = %slug, error = %e, "skip math during migration (GitHub pull failed)");
                continue;
            }
            match math::push(
                app,
                &ws.slug,
                &slug,
                dest.to_string_lossy().into_owned(),
                format!("migrated from GitHub team {}", team.name),
            )
            .await
            {
                Ok(_) => games += 1,
                Err(e) => {
                    tracing::warn!(game = %slug, error = %e, "failed to push migrated math")
                }
            }
        }
    }

    // Stamp the local team so the UI marks it migrated.
    crate::teams::mark_migrated(team_id, &ws.id).await.ok();

    Ok(MigrateReport {
        workspace_id: ws.id,
        workspace_slug: ws.slug,
        workspace_name: ws.name,
        profiles,
        rounds,
        games,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lgs::saved_rounds::SavedRound;

    #[test]
    fn github_profile_maps_to_doc_without_game_path() {
        // A legacy team profile JSON, including a machine path that must not
        // survive into the cloud document.
        let json = serde_json::json!({
            "id": "p1",
            "name": "Dice",
            "gamePath": "C:/somebody-else/dice",
            "gameUrl": "http://localhost:5174",
            "gameSlug": "dice-drop",
            "resolutions": [],
            "createdAt": 123u64,
            "updatedAt": 456u64,
            "teamId": "old-team"
        });
        let profile: Profile = serde_json::from_value(json).unwrap();
        let doc = profile_to_doc(&profile);
        let wire = serde_json::to_value(&doc).unwrap();
        assert!(wire.get("gamePath").is_none());
        assert!(wire.get("teamId").is_none());
        assert_eq!(doc.game_slug, "dice-drop");
        assert_eq!(doc.front_url.as_deref(), Some("http://localhost:5174"));
        assert_eq!(doc.created_at, 123);
        // Loose linkage is left unset for migrated profiles.
        assert_eq!(doc.game, None);
        assert_eq!(doc.revision, None);
    }

    #[test]
    fn legacy_round_maps_unpinned() {
        let round = SavedRound {
            id: "r1".into(),
            game_slug: "dice-drop".into(),
            mode: "base".into(),
            event_id: 7,
            description: "big win".into(),
            created_at: 100,
            updated_at: 200,
        };
        let doc = round_to_doc(&round);
        assert_eq!(doc.revision, None); // legacy = latest-at-the-time
        assert_eq!(doc.event_id, 7);
        assert_eq!(doc.game_slug, "dice-drop");
        assert_eq!(doc.created_at, 100);
    }
}
