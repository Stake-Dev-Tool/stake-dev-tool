//! The `gamePath` sidecar (contract decision #3).
//!
//! A synced `profile` document carries no filesystem path — `gamePath` is
//! per-machine and never leaves the desktop. The existing local `profiles.json`
//! doubles as the overlay store keyed by profile id: on pull we merge the cloud
//! [`ProfileDoc`] with the local `gamePath` for that id; on push we strip it.
//!
//! The merge/split are pure functions so they are unit-tested in isolation; the
//! overlay's persistence reuses [`crate::profiles`] (the same `profiles.json`).

use crate::profiles::Profile;

use super::documents::ProfileDoc;

/// Builds the desktop [`Profile`] shown in the UI from a cloud profile document
/// plus the local per-machine `gamePath` overlay for that id (empty when the
/// machine has never pulled/located the game).
///
/// `workspace_id` is stamped onto `team_id` so the UI groups the profile under
/// its workspace (the field is reused as the workspace/origin marker in M3).
pub fn doc_to_profile(
    workspace_id: &str,
    doc_id: &str,
    doc: ProfileDoc,
    local_game_path: Option<String>,
    updated_at_ms: u64,
) -> Profile {
    Profile {
        id: doc_id.to_string(),
        name: doc.name,
        game_path: local_game_path.unwrap_or_default(),
        game_url: doc.front_url.unwrap_or_default(),
        game_slug: doc.game_slug,
        resolutions: doc.resolutions,
        created_at: doc.created_at,
        updated_at: updated_at_ms,
        team_id: Some(workspace_id.to_string()),
    }
}

/// Splits a local [`Profile`] into the syncable [`ProfileDoc`], dropping the
/// per-machine `gamePath` and the local-origin `teamId`. `game`/`revision` are
/// the loose M2 linkage and are left unset here (the UI pins them explicitly).
pub fn profile_to_doc(profile: &Profile) -> ProfileDoc {
    ProfileDoc {
        name: profile.name.clone(),
        game_slug: profile.game_slug.clone(),
        game: None,
        revision: None,
        front_url: if profile.game_url.is_empty() {
            None
        } else {
            Some(profile.game_url.clone())
        },
        resolutions: profile.resolutions.clone(),
        created_at: profile.created_at,
    }
}

/// The local `gamePath` for a profile id, read from the overlay store. `None`
/// when there is no local record or its path is blank.
pub async fn local_game_path(profile_id: &str) -> Option<String> {
    let profiles = crate::profiles::list().await.ok()?;
    profiles
        .into_iter()
        .find(|p| p.id == profile_id)
        .map(|p| p.game_path)
        .filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lgs::settings::ResolutionPreset;

    fn sample_doc() -> ProfileDoc {
        ProfileDoc {
            name: "Dice Drop".into(),
            game_slug: "dice-drop".into(),
            game: Some("dice".into()),
            revision: Some(3),
            front_url: Some("http://localhost:5174".into()),
            resolutions: vec![ResolutionPreset {
                id: "desktop".into(),
                label: "Desktop".into(),
                width: 1200,
                height: 675,
                enabled: true,
                builtin: true,
            }],
            created_at: 1000,
        }
    }

    #[test]
    fn merge_overlays_local_game_path() {
        let p = doc_to_profile(
            "ws-1",
            "prof-1",
            sample_doc(),
            Some("C:/games/dice".into()),
            2000,
        );
        assert_eq!(p.id, "prof-1");
        assert_eq!(p.game_path, "C:/games/dice");
        assert_eq!(p.game_url, "http://localhost:5174");
        assert_eq!(p.game_slug, "dice-drop");
        assert_eq!(p.team_id.as_deref(), Some("ws-1"));
        assert_eq!(p.updated_at, 2000);
        assert_eq!(p.resolutions.len(), 1);
    }

    #[test]
    fn merge_without_overlay_leaves_game_path_empty() {
        let p = doc_to_profile("ws-1", "prof-1", sample_doc(), None, 2000);
        assert_eq!(p.game_path, "");
    }

    #[test]
    fn split_strips_game_path_and_team_id() {
        let profile = Profile {
            id: "prof-1".into(),
            name: "Dice Drop".into(),
            game_path: "C:/games/dice".into(),
            game_url: "http://localhost:5174".into(),
            game_slug: "dice-drop".into(),
            resolutions: vec![],
            created_at: 1000,
            updated_at: 2000,
            team_id: Some("ws-1".into()),
        };
        let doc = profile_to_doc(&profile);
        // The serialized document must never contain a path or team id.
        let json = serde_json::to_value(&doc).unwrap();
        assert!(json.get("gamePath").is_none());
        assert!(json.get("game_path").is_none());
        assert!(json.get("teamId").is_none());
        assert!(json.get("team_id").is_none());
        assert_eq!(doc.front_url.as_deref(), Some("http://localhost:5174"));
        assert_eq!(doc.game_slug, "dice-drop");
        assert_eq!(doc.created_at, 1000);
    }

    #[test]
    fn split_maps_blank_front_url_to_none() {
        let profile = Profile {
            id: "p".into(),
            name: "n".into(),
            game_path: String::new(),
            game_url: String::new(),
            game_slug: "g".into(),
            resolutions: vec![],
            created_at: 1,
            updated_at: 1,
            team_id: None,
        };
        assert_eq!(profile_to_doc(&profile).front_url, None);
    }
}
