//! Cloud-backed replacement for the local GitHub "teams" registry (which V2
//! removed outright — cloud workspaces are the only team system now).
//!
//! The re-pointed `teams_*` commands delegate here. `list_workspaces` is the
//! server's source of truth; this module layers a thin local cache
//! (`workspaces.json`) over it holding the **active** workspace, an id→slug map
//! (the M2/M3 endpoints are slug-addressed while the UI passes ids), and the
//! last synced `seq` per workspace (for `?since_seq=` incremental pulls and SSE
//! reconnect).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use tokio::fs;

use protocol::Role;

use super::api::CloudClient;

/// A workspace as surfaced to the UI (the M3 replacement for the V1 `Team`).
#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub role: Role,
    #[serde(rename = "memberCount")]
    pub member_count: Option<u32>,
}

/// A cached workspace identity (id ↔ slug ↔ name ↔ role). Returned by
/// [`resolve`] so slug-addressed callers can look one up without a round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub role: Role,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default, rename = "activeWorkspace")]
    active_workspace: Option<String>,
    #[serde(default)]
    workspaces: Vec<CacheEntry>,
    /// workspace id → last synced `seq` (document sync cursor).
    #[serde(default)]
    seqs: HashMap<String, i64>,
}

fn cache_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("could not resolve local data dir"))?
        .join("stake-dev-tool");
    Ok(dir.join("workspaces.json"))
}

async fn load() -> Result<CacheFile> {
    let path = cache_path()?;
    if !fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(CacheFile::default());
    }
    let bytes = fs::read(&path).await.context("read workspaces.json")?;
    Ok(serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn save(file: &CacheFile) -> Result<()> {
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .context("create cache dir")?;
    }
    let bytes = serde_json::to_vec_pretty(file).context("serialize workspaces cache")?;
    fs::write(&path, bytes)
        .await
        .context("write workspaces.json")?;
    Ok(())
}

fn client() -> Result<CloudClient> {
    CloudClient::from_stored_token().map_err(|e| anyhow!("{e}"))
}

/// Live list of the caller's workspaces (server-authoritative), refreshing the
/// local id→slug cache as a side effect. Member counts are filled best-effort
/// (one detail request per workspace, bounded concurrency); a failure leaves
/// the count `None` rather than failing the whole list.
pub async fn list() -> Result<Vec<Workspace>> {
    let client = client()?;
    let resp = client.list_workspaces().await.map_err(|e| anyhow!("{e}"))?;

    // Member counts, concurrently and best-effort.
    let counts: HashMap<String, u32> = stream::iter(resp.workspaces.clone())
        .map(|ws| {
            let client = client.clone();
            async move {
                let count = client
                    .workspace_detail(&ws.slug)
                    .await
                    .ok()
                    .map(|d| d.members.len() as u32);
                (ws.id.to_string(), count)
            }
        })
        .buffer_unordered(4)
        .filter_map(|(id, c)| async move { c.map(|c| (id, c)) })
        .collect()
        .await;

    let workspaces: Vec<Workspace> = resp
        .workspaces
        .into_iter()
        .map(|ws| Workspace {
            id: ws.id.to_string(),
            slug: ws.slug,
            name: ws.name,
            role: ws.role,
            member_count: counts.get(&ws.id.to_string()).copied(),
        })
        .collect();

    // Refresh the cache's id→slug map, pruning workspaces we no longer belong to
    // and clearing a dangling active pointer.
    let mut cache = load().await?;
    cache.workspaces = workspaces
        .iter()
        .map(|w| CacheEntry {
            id: w.id.clone(),
            slug: w.slug.clone(),
            name: w.name.clone(),
            role: w.role,
        })
        .collect();
    if let Some(active) = &cache.active_workspace
        && !cache.workspaces.iter().any(|e| &e.id == active)
    {
        cache.active_workspace = cache.workspaces.first().map(|e| e.id.clone());
    } else if cache.active_workspace.is_none() {
        cache.active_workspace = cache.workspaces.first().map(|e| e.id.clone());
    }
    save(&cache).await?;

    Ok(workspaces)
}

/// The active workspace from the local cache (no network). `None` when signed
/// out or not a member of anything cached yet.
pub async fn active() -> Result<Option<Workspace>> {
    let cache = load().await?;
    let Some(id) = cache.active_workspace.clone() else {
        return Ok(None);
    };
    Ok(cache
        .workspaces
        .into_iter()
        .find(|e| e.id == id)
        .map(|e| Workspace {
            id: e.id,
            slug: e.slug,
            name: e.name,
            role: e.role,
            member_count: None,
        }))
}

/// Persists the active-workspace choice locally.
pub async fn set_active(id: Option<&str>) -> Result<()> {
    let mut cache = load().await?;
    cache.active_workspace = id.map(|s| s.to_string());
    save(&cache).await
}

/// Resolves a workspace id to its `slug` (+ name/role) using the cache, falling
/// back to a live list when the id is not cached.
pub async fn resolve(id: &str) -> Result<CacheEntry> {
    let cache = load().await?;
    if let Some(e) = cache.workspaces.iter().find(|e| e.id == id) {
        return Ok(e.clone());
    }
    // Not cached — refresh once.
    let _ = list().await?;
    let cache = load().await?;
    cache
        .workspaces
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow!("workspace not found"))
}

/// The slug for a workspace id (see [`resolve`]).
pub async fn resolve_slug(id: &str) -> Result<String> {
    Ok(resolve(id).await?.slug)
}

/// Last synced document `seq` for a workspace (0 when never synced).
pub async fn last_seq(id: &str) -> Result<i64> {
    Ok(load().await?.seqs.get(id).copied().unwrap_or(0))
}

/// Records the newest `seq` applied for a workspace.
pub async fn set_last_seq(id: &str, seq: i64) -> Result<()> {
    let mut cache = load().await?;
    cache.seqs.insert(id.to_string(), seq);
    save(&cache).await
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Creates a workspace (caller becomes owner) and makes it active.
pub async fn create(name: &str, slug: Option<&str>) -> Result<Workspace> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("workspace name is required"));
    }
    let slug = match slug.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => slugify(s),
        None => slugify(name),
    };
    let summary = client()?
        .create_workspace(name, &slug)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let ws = Workspace {
        id: summary.id.to_string(),
        slug: summary.slug,
        name: summary.name,
        role: summary.role,
        member_count: Some(1),
    };
    // Refresh cache + activate.
    let _ = list().await;
    set_active(Some(&ws.id)).await.ok();
    Ok(ws)
}

/// Accepts an invite token, joining its workspace and making it active.
pub async fn join(token: &str) -> Result<Workspace> {
    let token = token.trim();
    if token.is_empty() {
        return Err(anyhow!("invite token is required"));
    }
    let resp = client()?
        .accept_invite(token)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let ws = Workspace {
        id: resp.workspace.id.to_string(),
        slug: resp.workspace.slug,
        name: resp.workspace.name,
        role: resp.workspace.role,
        member_count: None,
    };
    let _ = list().await;
    set_active(Some(&ws.id)).await.ok();
    Ok(ws)
}

/// Leaves a workspace: removes the caller's own membership on the server, then
/// drops it from the local cache/active pointer.
pub async fn leave(id: &str) -> Result<()> {
    let entry = resolve(id).await?;
    let client = client()?;
    // Identify self to remove the right membership row.
    let me = super::auth::current_user()
        .await?
        .ok_or_else(|| anyhow!("not signed in to cloud"))?;
    client
        .remove_member(&entry.slug, me.id)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    detach_local(id).await
}

/// Deletes a workspace (owner only), then drops it locally.
pub async fn delete(id: &str) -> Result<()> {
    let entry = resolve(id).await?;
    client()?
        .delete_workspace(&entry.slug)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    detach_local(id).await
}

/// Removes a workspace from the local cache and clears it as active.
async fn detach_local(id: &str) -> Result<()> {
    let mut cache = load().await?;
    cache.workspaces.retain(|e| e.id != id);
    cache.seqs.remove(id);
    if cache.active_workspace.as_deref() == Some(id) {
        cache.active_workspace = cache.workspaces.first().map(|e| e.id.clone());
    }
    save(&cache).await
}

/// Creates an invite for a workspace and returns its shareable URL.
pub async fn invite(id: &str, role: Role) -> Result<String> {
    let entry = resolve(id).await?;
    let created = client()?
        .create_invite(&entry.slug, role)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    Ok(created.invite_url)
}

/// Slugifies a name into a workspace slug (lowercase, dash-separated).
fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_v1_rules() {
        assert_eq!(slugify("My Slot Team"), "my-slot-team");
        assert_eq!(slugify("  --Foo__Bar!!  "), "foo-bar");
        assert_eq!(slugify("***"), "workspace");
        assert_eq!(slugify("already-a-slug"), "already-a-slug");
    }
}
