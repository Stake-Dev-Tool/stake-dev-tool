//! Push/pull math files between a local folder and a team GitHub repo.
//!
//! # Deprecated
//!
//! Part of the deprecated GitHub-repo team system. V2 replaces this with
//! content-addressed math revisions pushed to the cloud server (see `V2.md`,
//! milestone M2). Do not extend this module.
//!
//! The strategy:
//! - Metadata (file list + SHA-256 per chunk) lives at
//!   `math-manifests/<game-slug>.json` in the repo.
//! - Binaries live on a per-game GitHub Release tagged `math-<game-slug>`.
//!   Files larger than [`CHUNK_SIZE`] are split into `.partNNN` assets.
//! - Push is "last write wins": the local version overwrites remote assets if
//!   any chunk SHA differs.
//! - Pull downloads each chunk, reassembles, and verifies SHA.
//!
//! Limits / TODOs for a hardened version:
//! - No resume: a network blip mid-upload restarts the file.
//! - Each chunk is read fully into memory (~1 GiB). Future: stream from disk.
//! - No progress reporting back to the UI.
//! - No parallelism: chunks upload serially.

use anyhow::{Context, Result, anyhow};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How many chunks we upload at the same time within a single file. Higher
/// values saturate bandwidth faster but multiply RAM usage (each task buffers
/// a full chunk in memory) and hit GitHub rate limits sooner. 2 is a good
/// compromise — roughly doubles throughput over sequential without bumping
/// the risk profile.
const PARALLEL_CHUNKS: usize = 2;

use crate::github::api::{GithubClient, Release, ReleaseAsset};
use crate::teams::{self, Team};

pub const PROGRESS_EVENT: &str = "math-sync-progress";

#[derive(Debug, Clone, Serialize)]
pub struct MathSyncProgress {
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    /// "hashing" | "uploading" | "downloading" | "committing" | "done"
    pub phase: &'static str,
    #[serde(rename = "currentFile")]
    pub current_file: String,
    #[serde(rename = "fileIndex")]
    pub file_index: u32,
    #[serde(rename = "fileCount")]
    pub file_count: u32,
    #[serde(rename = "bytesDone")]
    pub bytes_done: u64,
    #[serde(rename = "bytesTotal")]
    pub bytes_total: u64,
}

fn emit_progress(app: &AppHandle, p: MathSyncProgress) {
    if let Err(e) = app.emit(PROGRESS_EVENT, &p) {
        tracing::warn!(error = %e, "failed to emit math-sync progress event");
    }
}

/// Max bytes per release asset. GitHub's published limit is 2 GiB, but large
/// uploads get flaky well before that. 1 GiB is a comfortable ceiling.
const CHUNK_SIZE: u64 = 1024 * 1024 * 1024;

const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MathManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Relative path from the game folder root (forward slashes).
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub chunks: Vec<ManifestChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestChunk {
    #[serde(rename = "assetName")]
    pub asset_name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MathSyncReport {
    #[serde(rename = "filesUploaded")]
    pub files_uploaded: u32,
    #[serde(rename = "filesSkipped")]
    pub files_skipped: u32,
    #[serde(rename = "chunksUploaded")]
    pub chunks_uploaded: u32,
    #[serde(rename = "bytesUploaded")]
    pub bytes_uploaded: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn team_by_id(team_id: &str) -> Result<Team> {
    let teams = teams::list_local().await?;
    teams
        .into_iter()
        .find(|t| t.id == team_id)
        .ok_or_else(|| anyhow!("team not found"))
}

fn manifest_path(game_slug: &str) -> String {
    format!("math-manifests/{game_slug}.json")
}

fn release_tag(game_slug: &str) -> String {
    format!("math-{game_slug}")
}

fn release_title(game_slug: &str) -> String {
    format!("Math: {game_slug}")
}

fn chunk_count(size: u64) -> u64 {
    if size == 0 {
        1
    } else {
        size.div_ceil(CHUNK_SIZE)
    }
}

fn chunk_asset_name(file_rel_path: &str, part_ix: u64, total_parts: u64, sha: &str) -> String {
    // GitHub releases have a flat asset namespace and reject duplicate names.
    // Include a short SHA suffix so a re-push with modified content uploads
    // under a new name — old assets stay valid for the previous manifest until
    // the new manifest is committed, which makes pushes crash-safe (an
    // interrupted upload never strands the remote state).
    let safe: String = file_rel_path
        .chars()
        .map(|c| if c == '/' || c == '\\' { '.' } else { c })
        .collect();
    let short = &sha[..8.min(sha.len())];
    if total_parts == 1 {
        format!("{safe}.{short}")
    } else {
        format!("{safe}.part{:03}.{short}", part_ix + 1)
    }
}

/// Walk the game folder and compute a manifest describing every file that
/// belongs in the repo. Uses relative, forward-slash paths.
async fn build_manifest(
    app: &AppHandle,
    game_slug: &str,
    game_path: &Path,
) -> Result<MathManifest> {
    // Pass 1: discover files + sizes so we have totals for the progress bar.
    let mut discovered: Vec<(PathBuf, String, u64)> = Vec::new();
    let mut stack = vec![game_path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = fs::read_dir(&dir)
            .await
            .with_context(|| format!("read_dir {}", dir.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let ft = entry.file_type().await?;
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(game_path)
                .context("strip prefix")?
                .to_string_lossy()
                .replace('\\', "/");
            let size = entry.metadata().await?.len();
            discovered.push((path, rel, size));
        }
    }
    discovered.sort_by(|a, b| a.1.cmp(&b.1));

    let total_bytes: u64 = discovered.iter().map(|(_, _, s)| *s).sum();
    let file_count = discovered.len() as u32;

    // Pass 2: hash each file, emitting progress.
    let mut files = Vec::with_capacity(discovered.len());
    let mut bytes_done: u64 = 0;
    for (ix, (path, rel, size)) in discovered.into_iter().enumerate() {
        emit_progress(
            app,
            MathSyncProgress {
                game_slug: game_slug.into(),
                phase: "hashing",
                current_file: rel.clone(),
                file_index: ix as u32,
                file_count,
                bytes_done,
                bytes_total: total_bytes,
            },
        );
        let (file_sha, chunks) = hash_and_chunk(&path, &rel, size).await?;
        bytes_done += size;
        files.push(ManifestFile {
            name: rel,
            size,
            sha256: file_sha,
            chunks,
        });
        emit_progress(
            app,
            MathSyncProgress {
                game_slug: game_slug.into(),
                phase: "hashing",
                current_file: "".into(),
                file_index: (ix as u32) + 1,
                file_count,
                bytes_done,
                bytes_total: total_bytes,
            },
        );
    }
    Ok(MathManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        game_slug: game_slug.to_string(),
        updated_at: now_ms(),
        files,
    })
}

/// Stream the file and compute both the whole-file SHA-256 and per-chunk
/// metadata in a single pass. Does not load the whole file into memory.
async fn hash_and_chunk(path: &Path, rel: &str, size: u64) -> Result<(String, Vec<ManifestChunk>)> {
    let mut file = fs::File::open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let mut whole = Sha256::new();
    let total_parts = chunk_count(size);
    let mut chunks = Vec::with_capacity(total_parts as usize);
    let mut buf = vec![0u8; 1024 * 1024]; // 1 MiB scratch buffer

    for part_ix in 0..total_parts {
        let mut part_hasher = Sha256::new();
        let mut part_read: u64 = 0;
        let target = if part_ix + 1 == total_parts {
            size - part_ix * CHUNK_SIZE
        } else {
            CHUNK_SIZE
        };
        while part_read < target {
            let to_read = ((target - part_read) as usize).min(buf.len());
            let n = file.read(&mut buf[..to_read]).await?;
            if n == 0 {
                break;
            }
            whole.update(&buf[..n]);
            part_hasher.update(&buf[..n]);
            part_read += n as u64;
        }
        let part_sha = hex(&part_hasher.finalize());
        chunks.push(ManifestChunk {
            asset_name: chunk_asset_name(rel, part_ix, total_parts, &part_sha),
            size: part_read,
            sha256: part_sha,
        });
    }
    let file_sha = hex(&whole.finalize());
    Ok((file_sha, chunks))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

async fn fetch_manifest(
    client: &GithubClient,
    team: &Team,
    game_slug: &str,
) -> Result<Option<(MathManifest, String)>> {
    let path = manifest_path(game_slug);
    let Some(file) = client
        .get_file(&team.repo_owner, &team.repo_name, &path)
        .await?
    else {
        return Ok(None);
    };
    let manifest: MathManifest = serde_json::from_slice(&file.content)
        .with_context(|| format!("parse manifest at {path}"))?;
    Ok(Some((manifest, file.sha)))
}

async fn ensure_release(client: &GithubClient, team: &Team, game_slug: &str) -> Result<Release> {
    let tag = release_tag(game_slug);
    if let Some(r) = client
        .find_release_by_tag(&team.repo_owner, &team.repo_name, &tag)
        .await?
    {
        return Ok(r);
    }
    client
        .create_release(
            &team.repo_owner,
            &team.repo_name,
            &tag,
            &release_title(game_slug),
            &format!("Math files for {game_slug}. Managed by stake-dev-tool."),
        )
        .await
}

async fn read_chunk_bytes(path: &Path, part_ix: u64, total_parts: u64) -> Result<Vec<u8>> {
    let mut file = fs::File::open(path).await?;
    let offset = part_ix * CHUNK_SIZE;
    if offset > 0 {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
    }
    let meta = fs::metadata(path).await?;
    let size = meta.len();
    let target = if part_ix + 1 == total_parts {
        size - offset
    } else {
        CHUNK_SIZE
    };
    let mut out = Vec::with_capacity(target as usize);
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let mut read_total: u64 = 0;
    while read_total < target {
        let to_read = ((target - read_total) as usize).min(buf.len());
        let n = file.read(&mut buf[..to_read]).await?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        read_total += n as u64;
    }
    Ok(out)
}

pub async fn push(
    app: &AppHandle,
    team_id: &str,
    game_slug: &str,
    game_path: String,
) -> Result<MathSyncReport> {
    let team = team_by_id(team_id).await?;
    let client = GithubClient::from_stored_token()?;
    let game_path = PathBuf::from(&game_path);
    if !game_path.is_dir() {
        return Err(anyhow!(
            "game path is not a directory: {}",
            game_path.display()
        ));
    }

    emit_progress(
        app,
        MathSyncProgress {
            game_slug: game_slug.into(),
            phase: "hashing",
            current_file: "(scanning)".into(),
            file_index: 0,
            file_count: 0,
            bytes_done: 0,
            bytes_total: 0,
        },
    );

    let local_manifest = build_manifest(app, game_slug, &game_path).await?;
    let remote = fetch_manifest(&client, &team, game_slug).await?;
    let remote_manifest = remote.as_ref().map(|(m, _)| m);
    let manifest_prev_sha = remote.as_ref().map(|(_, s)| s.clone());

    let release = ensure_release(&client, &team, game_slug).await?;
    let existing_assets: std::collections::HashMap<String, &ReleaseAsset> =
        release.assets.iter().map(|a| (a.name.clone(), a)).collect();

    // A file is only "in sync" if BOTH the SHA matches AND every chunk named
    // in the remote manifest is actually present on the release. We verify
    // asset presence because a remote can end up with a stale manifest but
    // missing binaries (interrupted push, manually deleted assets, release
    // recreated, …). Trusting the manifest alone gives false "already in
    // sync" reports.
    let is_in_sync = |lf: &ManifestFile| -> bool {
        let Some(rf) = remote_manifest.and_then(|m| m.files.iter().find(|f| f.name == lf.name))
        else {
            return false;
        };
        if rf.sha256 != lf.sha256 {
            return false;
        }
        rf.chunks
            .iter()
            .all(|c| existing_assets.contains_key(&c.asset_name))
    };

    let planned_total_bytes: u64 = local_manifest
        .files
        .iter()
        .filter(|lf| !is_in_sync(lf))
        .map(|f| f.size)
        .sum();

    let mut files_uploaded = 0u32;
    let mut files_skipped = 0u32;
    let mut chunks_uploaded = 0u32;
    let mut bytes_uploaded = 0u64;
    let file_count = local_manifest.files.len() as u32;

    for (file_ix, local_file) in local_manifest.files.iter().enumerate() {
        if is_in_sync(local_file) {
            tracing::debug!(file = %local_file.name, "skip: SHA matches and all chunks present");
            files_skipped += 1;
            continue;
        }
        tracing::info!(
            file = %local_file.name,
            size = local_file.size,
            "uploading (sha mismatch or missing chunks on release)"
        );

        emit_progress(
            app,
            MathSyncProgress {
                game_slug: game_slug.into(),
                phase: "uploading",
                current_file: local_file.name.clone(),
                file_index: file_ix as u32,
                file_count,
                bytes_done: bytes_uploaded,
                bytes_total: planned_total_bytes,
            },
        );

        let file_abs = game_path.join(&local_file.name);
        let total = local_file.chunks.len() as u64;

        // Count chunks already on the release (identical SHA-suffixed name =
        // byte-identical content, so upload is a no-op). Only the missing
        // ones go through the parallel pipeline.
        let mut to_upload: Vec<(u64, ManifestChunk)> = Vec::new();
        for (ix, chunk) in local_file.chunks.iter().enumerate() {
            if existing_assets.contains_key(&chunk.asset_name) {
                chunks_uploaded += 1;
                bytes_uploaded += chunk.size;
            } else {
                to_upload.push((ix as u64, chunk.clone()));
            }
        }

        if !to_upload.is_empty() {
            let client_for_tasks = client.clone();
            let upload_url = release.upload_url.clone();
            let file_abs_for_tasks = file_abs.clone();

            let stream = stream::iter(to_upload)
                .map(|(ix, chunk)| {
                    let client = client_for_tasks.clone();
                    let upload_url = upload_url.clone();
                    let file_abs = file_abs_for_tasks.clone();
                    async move {
                        let bytes = read_chunk_bytes(&file_abs, ix, total).await?;
                        client
                            .upload_release_asset(&upload_url, &chunk.asset_name, bytes)
                            .await
                            .with_context(|| format!("upload {}", chunk.asset_name))?;
                        Ok::<u64, anyhow::Error>(chunk.size)
                    }
                })
                .buffer_unordered(PARALLEL_CHUNKS);

            tokio::pin!(stream);
            while let Some(res) = stream.next().await {
                let size = res?;
                chunks_uploaded += 1;
                bytes_uploaded += size;

                emit_progress(
                    app,
                    MathSyncProgress {
                        game_slug: game_slug.into(),
                        phase: "uploading",
                        current_file: local_file.name.clone(),
                        file_index: file_ix as u32,
                        file_count,
                        bytes_done: bytes_uploaded,
                        bytes_total: planned_total_bytes,
                    },
                );
            }
        }
        files_uploaded += 1;
    }

    let local_asset_names: std::collections::HashSet<&str> = local_manifest
        .files
        .iter()
        .flat_map(|f| f.chunks.iter().map(|c| c.asset_name.as_str()))
        .collect();
    for a in &release.assets {
        if !local_asset_names.contains(a.name.as_str()) {
            client
                .delete_release_asset(&team.repo_owner, &team.repo_name, a.id)
                .await
                .ok();
        }
    }

    emit_progress(
        app,
        MathSyncProgress {
            game_slug: game_slug.into(),
            phase: "committing",
            current_file: ".stake-team manifest".into(),
            file_index: file_count,
            file_count,
            bytes_done: bytes_uploaded,
            bytes_total: planned_total_bytes,
        },
    );

    let bytes = serde_json::to_vec_pretty(&local_manifest)?;
    client
        .put_file(
            &team.repo_owner,
            &team.repo_name,
            &manifest_path(game_slug),
            &bytes,
            &format!("sync: math manifest for {game_slug}"),
            manifest_prev_sha.as_deref(),
        )
        .await?;

    emit_progress(
        app,
        MathSyncProgress {
            game_slug: game_slug.into(),
            phase: "done",
            current_file: "".into(),
            file_index: file_count,
            file_count,
            bytes_done: bytes_uploaded,
            bytes_total: planned_total_bytes,
        },
    );

    Ok(MathSyncReport {
        files_uploaded,
        files_skipped,
        chunks_uploaded,
        bytes_uploaded,
    })
}

pub async fn pull(
    app: &AppHandle,
    team_id: &str,
    game_slug: &str,
    dest_path: String,
) -> Result<MathSyncReport> {
    let team = team_by_id(team_id).await?;
    let client = GithubClient::from_stored_token()?;
    let dest = PathBuf::from(&dest_path);
    fs::create_dir_all(&dest)
        .await
        .with_context(|| format!("create dest {}", dest.display()))?;

    let (manifest, _sha) = fetch_manifest(&client, &team, game_slug)
        .await?
        .ok_or_else(|| anyhow!("no manifest for {game_slug} in this team"))?;
    let release = client
        .find_release_by_tag(&team.repo_owner, &team.repo_name, &release_tag(game_slug))
        .await?
        .ok_or_else(|| anyhow!("no release for {game_slug} in this team"))?;
    let assets: std::collections::HashMap<String, &ReleaseAsset> =
        release.assets.iter().map(|a| (a.name.clone(), a)).collect();

    // Pre-flight: list every chunk the manifest expects and confirm each one
    // is actually on the release. If any are missing, fail fast with a clear
    // message before the user watches a progress bar for files that were
    // never going to succeed.
    let missing: Vec<&str> = manifest
        .files
        .iter()
        .flat_map(|f| f.chunks.iter().map(|c| c.asset_name.as_str()))
        .filter(|name| !assets.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        return Err(anyhow!(
            "Remote release for '{}' is inconsistent with its manifest. \
             {} chunk(s) are referenced but not uploaded (first missing: '{}'). \
             Ask whoever pushed this game to push it again — the previous push \
             probably failed mid-upload.",
            game_slug,
            missing.len(),
            missing[0]
        ));
    }

    let planned_total_bytes: u64 = manifest.files.iter().map(|f| f.size).sum();
    let file_count = manifest.files.len() as u32;

    let mut files_uploaded = 0u32;
    let mut files_skipped = 0u32;
    let mut chunks_uploaded = 0u32;
    let mut bytes_uploaded = 0u64;

    for (file_ix, f) in manifest.files.iter().enumerate() {
        let out = dest.join(&f.name);
        if let Ok(meta) = fs::metadata(&out).await
            && meta.len() == f.size
        {
            let (existing_sha, _) = hash_and_chunk(&out, &f.name, f.size).await?;
            if existing_sha == f.sha256 {
                files_skipped += 1;
                bytes_uploaded += f.size;
                continue;
            }
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).await?;
        }

        emit_progress(
            app,
            MathSyncProgress {
                game_slug: game_slug.into(),
                phase: "downloading",
                current_file: f.name.clone(),
                file_index: file_ix as u32,
                file_count,
                bytes_done: bytes_uploaded,
                bytes_total: planned_total_bytes,
            },
        );

        let mut file = fs::File::create(&out).await?;
        let mut whole = Sha256::new();
        for chunk in &f.chunks {
            let asset = assets.get(&chunk.asset_name).ok_or_else(|| {
                let available = assets
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow!(
                    "asset '{}' for file '{}' not found in release '{}'. Available: [{}]. \
                     This usually means a previous push failed mid-upload. Ask the owner to re-push.",
                    chunk.asset_name,
                    f.name,
                    release_tag(game_slug),
                    if available.is_empty() { "<empty release>" } else { &available }
                )
            })?;
            let bytes = client.download_release_asset(&asset.url).await?;
            let mut h = Sha256::new();
            h.update(&bytes);
            let got = hex(&h.finalize());
            if got != chunk.sha256 {
                return Err(anyhow!(
                    "SHA mismatch for {}: expected {}, got {}",
                    chunk.asset_name,
                    chunk.sha256,
                    got
                ));
            }
            whole.update(&bytes);
            file.write_all(&bytes).await?;
            chunks_uploaded += 1;
            bytes_uploaded += bytes.len() as u64;

            emit_progress(
                app,
                MathSyncProgress {
                    game_slug: game_slug.into(),
                    phase: "downloading",
                    current_file: f.name.clone(),
                    file_index: file_ix as u32,
                    file_count,
                    bytes_done: bytes_uploaded,
                    bytes_total: planned_total_bytes,
                },
            );
        }
        file.flush().await?;
        let final_sha = hex(&whole.finalize());
        if final_sha != f.sha256 {
            return Err(anyhow!(
                "whole-file SHA mismatch for {}: expected {}, got {}",
                f.name,
                f.sha256,
                final_sha
            ));
        }
        files_uploaded += 1;
    }

    emit_progress(
        app,
        MathSyncProgress {
            game_slug: game_slug.into(),
            phase: "done",
            current_file: "".into(),
            file_index: file_count,
            file_count,
            bytes_done: bytes_uploaded,
            bytes_total: planned_total_bytes,
        },
    );

    Ok(MathSyncReport {
        files_uploaded,
        files_skipped,
        chunks_uploaded,
        bytes_uploaded,
    })
}

pub async fn list_remote_games(team_id: &str) -> Result<Vec<String>> {
    let team = team_by_id(team_id).await?;
    let client = GithubClient::from_stored_token()?;
    let entries = client
        .list_dir(&team.repo_owner, &team.repo_name, "math-manifests")
        .await?;
    let mut out = Vec::new();
    for e in entries {
        if e.kind == "file" && e.name.ends_with(".json") {
            out.push(e.name.trim_end_matches(".json").to_string());
        }
    }
    out.sort();
    Ok(out)
}
