//! Per-revision bet stats — the M2 differentiator.
//!
//! After a revision commits, [`compute_stats_for_revision`] materializes *only*
//! the small files a mode's stats need (`index.json` and the lookup-table CSVs
//! its modes reference as `weights`) from the object store into a temp dir laid
//! out as `<tmp>/<game_slug>/…`, then computes per-mode RTP / max-win / entries.
//! Books (`events` files) are **never** downloaded.
//!
//! ## Reuse of `lgs`
//! `index.json` is parsed through the real engine path the multi-tenant LGS
//! (M4) will use: a throwaway [`lgs::TenantId`] over the temp dir via
//! [`lgs::TenantRegistry::get_or_create_disk`], then
//! [`lgs::math_engine::MathEngine::load_config`]. That is the highest-level lgs
//! API that fits — it owns the `index.json` contract (`GameMode { name, cost,
//! events, weights }`, integer-or-float cost handling, nested file resolution).
//!
//! The weighted RTP itself is computed here rather than in `lgs`: lgs's only
//! public weights API, `MathEngine::game_bet_stats`, returns *notable-bet ids*
//! (zero/low/medium/big/max), not aggregate RTP, and its underlying
//! `parse_weights` is private. We therefore parse the same 3-column lookup
//! format lgs consumes (`event_id,weight,payout_multiplier`, the payout column
//! scaled ×100 exactly as lgs's `build_result` divides by 100) and derive RTP
//! from weights alone.

use std::sync::Arc;

use object_store::ObjectStore;
use protocol::ModeStats;
use sqlx::PgPool;
use uuid::Uuid;

use crate::blobs;

/// Compute and persist a revision's per-mode bet stats. Always leaves a terminal
/// `revision_stats` row: `ok` with `{modes:[...]}`, or `error` with a message.
/// Never panics or propagates — the commit endpoint fires it via `tokio::spawn`,
/// and integration tests `await` it directly for deterministic assertions.
pub async fn compute_stats_for_revision(
    pool: PgPool,
    store: Arc<dyn ObjectStore>,
    revision_id: Uuid,
) {
    if let Err(e) = set_pending(&pool, revision_id).await {
        tracing::error!(error = %e, %revision_id, "stats: could not mark pending");
        return;
    }
    match compute(&pool, store.as_ref(), revision_id).await {
        Ok(modes) => {
            let data = serde_json::json!({ "modes": modes });
            if let Err(e) = finalize(&pool, revision_id, "ok", None, Some(data)).await {
                tracing::error!(error = %e, %revision_id, "stats: could not store ok result");
            }
        }
        Err(e) => {
            let message = format!("{e:#}");
            tracing::warn!(error = %message, %revision_id, "stats: computation failed");
            if let Err(e) = finalize(&pool, revision_id, "error", Some(&message), None).await {
                tracing::error!(error = %e, %revision_id, "stats: could not store error result");
            }
        }
    }
}

/// Upsert a `pending` row so an in-progress (or crashed) computation is visible.
async fn set_pending(pool: &PgPool, revision_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO revision_stats (revision_id, status, error, data, updated_at) \
         VALUES ($1, 'pending', NULL, NULL, now()) \
         ON CONFLICT (revision_id) \
         DO UPDATE SET status = 'pending', error = NULL, data = NULL, updated_at = now()",
    )
    .bind(revision_id)
    .execute(pool)
    .await
    .map(|_| ())
}

async fn finalize(
    pool: &PgPool,
    revision_id: Uuid,
    status: &str,
    error: Option<&str>,
    data: Option<serde_json::Value>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE revision_stats \
         SET status = $2, error = $3, data = $4, updated_at = now() \
         WHERE revision_id = $1",
    )
    .bind(revision_id)
    .bind(status)
    .bind(error)
    .bind(data)
    .execute(pool)
    .await
    .map(|_| ())
}

async fn compute(
    pool: &PgPool,
    store: &dyn ObjectStore,
    revision_id: Uuid,
) -> anyhow::Result<Vec<ModeStats>> {
    let (workspace_id, game_slug): (Uuid, String) = sqlx::query_as(
        "SELECT g.workspace_id, g.slug FROM revisions r \
         JOIN games g ON g.id = r.game_id WHERE r.id = $1",
    )
    .bind(revision_id)
    .fetch_one(pool)
    .await?;

    // path -> hash (BYTEA) for every file in the revision.
    let files: Vec<(String, Vec<u8>)> =
        sqlx::query_as("SELECT path, hash FROM revision_files WHERE revision_id = $1")
            .bind(revision_id)
            .fetch_all(pool)
            .await?;
    let hex_for = |path: &str| -> Option<String> {
        files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, h)| blobs::to_hex(h))
    };

    let tmp = tempfile::tempdir()?;
    let game_dir = tmp.path().join(&game_slug);
    tokio::fs::create_dir_all(&game_dir).await?;

    // Materialize index.json (the manifest guarantees exactly one at the root).
    let index_hex =
        hex_for("index.json").ok_or_else(|| anyhow::anyhow!("revision has no root index.json"))?;
    let index_bytes = blobs::fetch_blob_vec(store, workspace_id, &index_hex).await?;
    tokio::fs::write(game_dir.join("index.json"), &index_bytes).await?;

    // Parse index.json via the real lgs engine over the temp dir as math root.
    let registry = lgs::TenantRegistry::new();
    let tenant = lgs::TenantId::from(format!("stats-{revision_id}"));
    let state = registry.get_or_create_disk(tenant, tmp.path());
    let cfg = state.engine.load_config(&game_slug).await?;

    let mut modes = Vec::with_capacity(cfg.modes.len());
    for mode in &cfg.modes {
        // Only the weights CSV is fetched; the `events` (books) file is not.
        let weights_hex = hex_for(&mode.weights).ok_or_else(|| {
            anyhow::anyhow!(
                "mode \"{}\" references weights \"{}\" not present in the revision",
                mode.name,
                mode.weights
            )
        })?;
        let weights_bytes = blobs::fetch_blob_vec(store, workspace_id, &weights_hex).await?;

        // Materialize into the temp dir for a faithful `<tmp>/<game>/…` layout,
        // then compute from the same bytes (avoids a redundant re-read).
        let dest = game_dir.join(&mode.weights);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&dest, &weights_bytes).await?;

        let text = String::from_utf8(weights_bytes)
            .map_err(|e| anyhow::anyhow!("weights \"{}\" is not utf-8: {e}", mode.weights))?;
        modes.push(compute_mode_stats(&mode.name, mode.cost, &text)?);
    }
    Ok(modes)
}

/// Compute one mode's stats from its lookup-table CSV. Mirrors lgs's
/// `parse_weights` format: one `event_id,weight,payout_multiplier` row per line
/// (blank lines skipped). The payout column is in hundredths (100 == 1.00×),
/// matching lgs's `build_result`, so the decimal win multiple is `payout / 100`.
///
/// * `rtp`     = `sum(weight * payout/100) / sum(weight) / cost`
/// * `max_win` = `max(payout) / 100`
/// * `entries` = number of rows
/// * `hit_rate`= share of total weight with a non-zero payout
fn compute_mode_stats(mode: &str, cost: u64, weights_csv: &str) -> anyhow::Result<ModeStats> {
    let mut total_weight: u128 = 0;
    let mut weighted_payout: u128 = 0; // sum(weight * payout_multiplier)
    let mut win_weight: u128 = 0; // sum(weight) over rows with payout > 0
    let mut max_payout: u32 = 0;
    let mut entries: u64 = 0;

    for (i, line) in weights_csv.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split(',');
        let lineno = i + 1;
        // event_id is validated (format fidelity with lgs) though unused here.
        let _event_id: u32 = it
            .next()
            .ok_or_else(|| anyhow::anyhow!("weights line {lineno}: missing event id"))?
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("weights line {lineno}: bad event id: {e}"))?;
        let weight: u64 = it
            .next()
            .ok_or_else(|| anyhow::anyhow!("weights line {lineno}: missing weight"))?
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("weights line {lineno}: bad weight: {e}"))?;
        let payout: u32 = it
            .next()
            .ok_or_else(|| anyhow::anyhow!("weights line {lineno}: missing payout"))?
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("weights line {lineno}: bad payout: {e}"))?;

        total_weight += weight as u128;
        weighted_payout += weight as u128 * payout as u128;
        if payout > 0 {
            win_weight += weight as u128;
        }
        max_payout = max_payout.max(payout);
        entries += 1;
    }

    if entries == 0 {
        anyhow::bail!("lookup table has no rows");
    }
    if total_weight == 0 {
        anyhow::bail!("lookup table has zero total weight");
    }

    let cost_f = cost.max(1) as f64;
    let rtp = (weighted_payout as f64) / 100.0 / (total_weight as f64) / cost_f;
    let max_win = (max_payout as f64) / 100.0;
    let hit_rate = (win_weight as f64) / (total_weight as f64);

    Ok(ModeStats {
        mode: mode.to_string(),
        cost: cost as f64,
        rtp,
        max_win,
        entries,
        hit_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_and_max_win_match_hand_computed_values() {
        // total weight 10000; sum(weight*payout) = 960000.
        // rtp = 960000 / 100 / 10000 / 1 = 0.96
        // max_win = 42000 / 100 = 420.0 ; hit_rate = 1000/10000 = 0.10
        let csv = "0,9000,0\n1,900,100\n2,90,5000\n3,10,42000\n";
        let s = compute_mode_stats("base", 1, csv).expect("stats");
        assert_eq!(s.mode, "base");
        assert_eq!(s.cost, 1.0);
        assert!((s.rtp - 0.96).abs() < 1e-9, "rtp = {}", s.rtp);
        assert!((s.max_win - 420.0).abs() < 1e-9, "max_win = {}", s.max_win);
        assert_eq!(s.entries, 4);
        assert!(
            (s.hit_rate - 0.10).abs() < 1e-9,
            "hit_rate = {}",
            s.hit_rate
        );
    }

    #[test]
    fn cost_divides_rtp() {
        // Same table but cost 10 → rtp is a tenth.
        let csv = "0,9000,0\n1,900,100\n2,90,5000\n3,10,42000\n";
        let s = compute_mode_stats("bonus", 10, csv).expect("stats");
        assert_eq!(s.cost, 10.0);
        assert!((s.rtp - 0.096).abs() < 1e-9, "rtp = {}", s.rtp);
    }

    #[test]
    fn blank_lines_are_skipped() {
        let s = compute_mode_stats("base", 1, "\n0,1,0\n\n1,1,200\n").expect("stats");
        assert_eq!(s.entries, 2);
    }

    #[test]
    fn malformed_csv_is_an_error() {
        assert!(compute_mode_stats("base", 1, "not,a,number\n").is_err());
        assert!(compute_mode_stats("base", 1, "0,1\n").is_err()); // missing payout
        assert!(compute_mode_stats("base", 1, "").is_err()); // no rows
    }
}
