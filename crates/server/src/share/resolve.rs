//! Read-side resolution for the share host: slug -> link, link -> revision, and
//! link -> front bundle. Each "not available" outcome maps to a branded page
//! (via [`super::pages`]) rather than an error envelope, since these are served
//! straight to a browser on the share subdomain.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiResult;

use super::pages;

/// A resolved, *currently valid* share link plus its game slug.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct ResolvedShare {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub game_id: Uuid,
    pub game_slug: String,
    pub revision_number: Option<i32>,
    pub front_bundle_id: Option<Uuid>,
    pub password_hash: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_concurrent_sessions: i32,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ResolvedShare {
    pub(super) fn is_password_protected(&self) -> bool {
        self.password_hash.is_some()
    }
}

/// Load a link by its subdomain slug (does not check validity).
async fn load_by_slug(pool: &PgPool, slug: &str) -> ApiResult<Option<ResolvedShare>> {
    let row = sqlx::query_as::<_, ResolvedShare>(
        "SELECT s.id, s.workspace_id, s.game_id, g.slug AS game_slug, s.revision_number, \
                s.front_bundle_id, s.password_hash, s.expires_at, s.max_concurrent_sessions, \
                s.revoked_at \
         FROM share_links s JOIN games g ON g.id = s.game_id \
         WHERE s.slug = $1",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Resolve a link by slug and enforce validity. On any failure returns the
/// branded page to serve (unknown/revoked -> 404, expired -> expired page). A DB
/// error maps to the internal page (logged) rather than propagating.
pub(super) async fn resolve(
    pool: &PgPool,
    slug: &str,
) -> Result<ResolvedShare, axum::response::Response> {
    let link = match load_by_slug(pool, slug).await {
        Ok(link) => link,
        Err(e) => {
            tracing::error!(error = %e, slug, "share: failed to load link");
            return Err(pages::internal());
        }
    };
    let Some(link) = link else {
        return Err(pages::not_found());
    };
    if link.revoked_at.is_some() {
        // A revoked link is indistinguishable from an unknown one.
        return Err(pages::not_found());
    }
    if let Some(expires_at) = link.expires_at
        && expires_at <= Utc::now()
    {
        return Err(pages::expired());
    }
    Ok(link)
}

/// Resolve the revision number + id this link plays against (pinned, or the
/// game's latest). Returns the "no revision yet" page when there is none.
pub(super) async fn resolve_revision(
    pool: &PgPool,
    link: &ResolvedShare,
) -> Result<(i32, Uuid), axum::response::Response> {
    let number = match link.revision_number {
        Some(number) => number,
        None => {
            let latest: Option<i32> =
                match sqlx::query_scalar("SELECT MAX(number) FROM revisions WHERE game_id = $1")
                    .bind(link.game_id)
                    .fetch_one(pool)
                    .await
                {
                    Ok(latest) => latest,
                    Err(e) => {
                        tracing::error!(error = %e, "share: failed to resolve latest revision");
                        return Err(pages::internal());
                    }
                };
            match latest {
                Some(number) => number,
                None => return Err(pages::no_revision()),
            }
        }
    };

    let revision_id: Option<Uuid> =
        match sqlx::query_scalar("SELECT id FROM revisions WHERE game_id = $1 AND number = $2")
            .bind(link.game_id)
            .bind(number)
            .fetch_optional(pool)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(error = %e, "share: failed to resolve revision id");
                return Err(pages::internal());
            }
        };
    match revision_id {
        Some(id) => Ok((number, id)),
        None => Err(pages::no_revision()),
    }
}

/// A resolved front bundle: its id and parsed `path -> {hash, size}` manifest.
pub(super) struct ResolvedBundle {
    pub entries: std::collections::HashMap<String, BundleEntry>,
}

pub(super) struct BundleEntry {
    pub hash: String,
    pub size: i64,
}

/// Resolve the front bundle this link serves (pinned, or the game's latest).
/// Returns the "no front build yet" page when there is none.
pub(super) async fn resolve_bundle(
    pool: &PgPool,
    link: &ResolvedShare,
) -> Result<ResolvedBundle, axum::response::Response> {
    let manifest: Option<Value> = match link.front_bundle_id {
        Some(id) => {
            match sqlx::query_scalar(
                "SELECT manifest FROM front_bundles WHERE id = $1 AND game_id = $2",
            )
            .bind(id)
            .bind(link.game_id)
            .fetch_optional(pool)
            .await
            {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "share: failed to load pinned bundle");
                    return Err(pages::internal());
                }
            }
        }
        None => {
            match sqlx::query_scalar(
                "SELECT manifest FROM front_bundles WHERE game_id = $1 \
                 ORDER BY created_at DESC, id DESC LIMIT 1",
            )
            .bind(link.game_id)
            .fetch_optional(pool)
            .await
            {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "share: failed to load latest bundle");
                    return Err(pages::internal());
                }
            }
        }
    };

    let Some(manifest) = manifest else {
        return Err(pages::no_bundle());
    };
    match parse_manifest(&manifest) {
        Some(entries) => Ok(ResolvedBundle { entries }),
        None => {
            tracing::error!("share: bundle manifest is malformed");
            Err(pages::internal())
        }
    }
}

/// Parse a stored `{ "<path>": { "hash": "<hex>", "size": <int> } }` manifest.
fn parse_manifest(value: &Value) -> Option<std::collections::HashMap<String, BundleEntry>> {
    let obj = value.as_object()?;
    let mut out = std::collections::HashMap::with_capacity(obj.len());
    for (path, entry) in obj {
        let hash = entry.get("hash")?.as_str()?.to_string();
        let size = entry.get("size")?.as_i64()?;
        out.insert(path.clone(), BundleEntry { hash, size });
    }
    Some(out)
}
