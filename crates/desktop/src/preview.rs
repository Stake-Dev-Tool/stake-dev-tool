//! Build + publish a browser-side preview of a profile.
//!
//! End result: `https://<user>.github.io/stake-dev-tool-previews/<slug>/` —
//! a static page that runs the game with the WASM RGS in place of the local
//! one. Pipeline:
//!
//! 1. Assemble a bundle directory locally: harness HTML/JS + compiled WASM
//!    (embedded) + the user's built front + math files.
//! 2. Ensure a public repo `<user>/stake-dev-tool-previews` exists.
//! 3. Upload every bundle file to the repo via the Contents API.
//! 4. Enable Pages (idempotent).
//! 5. Return the URL.
//!
//! Math is currently uploaded as plain files. That works up to GitHub's
//! 100 MB-per-file / 1 GB-per-repo limits — small/medium games. Big games
//! will need chunked Release assets which is a follow-up.

use anyhow::{Context, Result, anyhow};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};
use tokio::fs;

use crate::cloud::math::{MathSyncProgress, PROGRESS_EVENT};
use crate::file_chunker;
use crate::github::api::{GitTreeEntry, GithubClient};
use crate::profiles::Profile;
use futures_util::stream::{self, StreamExt};

#[allow(clippy::too_many_arguments)]
fn emit_progress(
    app: &AppHandle,
    slug: &str,
    phase: &'static str,
    current_file: &str,
    file_index: u32,
    file_count: u32,
    bytes_done: u64,
    bytes_total: u64,
) {
    let _ = app.emit(
        PROGRESS_EVENT,
        MathSyncProgress {
            game_slug: slug.to_string(),
            phase,
            current_file: current_file.to_string(),
            file_index,
            file_count,
            bytes_done,
            bytes_total,
        },
    );
}

/// What gets uploaded as the math payload of a preview. Anything reachable
/// from a public Pages URL is visible to anyone — there's no way to ship a
/// playable browser preview AND keep the math secret. The choice is between
/// shipping it intact, a halved subset, or a small representative sample.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MathMode {
    /// Ship the math files exactly as they sit on disk. Preview plays
    /// normally, RTP unchanged. Math is fully public.
    Full,
    /// Truncate every book's `events` array to its first half. Preview
    /// still plays for casual checks but RTP / pacing / animations break,
    /// so reverse-engineering the production weights is much harder.
    Partial,
    /// Pick ~100 books per mode with a curated payout distribution
    /// (no-wins + max + average + spread tiers). Preview is tiny (a few MB),
    /// publishes fast, plays end-to-end with limited variety. Best for a
    /// playable demo link.
    #[default]
    Sampled,
}

/// Files copied verbatim into every preview: the harness HTML, runtime JS,
/// and the example bundle.json (the real one is generated per-preview).
static PREVIEW_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../lgs-wasm/preview-template");

/// `wasm-pack`-built JS glue + .wasm. Refresh by re-running wasm-pack before
/// each Tauri build. The macro re-scans the directory whenever this source
/// file changes, which is why edits to lgs-wasm need a touch here too. (v3)
static WASM_PKG: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../lgs-wasm/pkg");

/// Per-preview repos are created on demand under the user's account so each
/// preview gets its own ~1 GB Pages budget. The shared
/// `stake-dev-tool-previews` of v1 ran into the limit fast and made unpublish
/// finicky (deleting subfolders via Contents API, slow).
fn preview_repo_name(slug_label: &str) -> String {
    format!("stake-dev-tool-preview-{slug_label}")
}

#[derive(Debug, Clone, Serialize)]
pub struct PublishReport {
    pub url: String,
    #[serde(rename = "filesUploaded")]
    pub files_uploaded: u32,
    #[serde(rename = "filesSkipped")]
    pub files_skipped: u32,
    #[serde(rename = "bytesUploaded")]
    pub bytes_uploaded: u64,
}

#[derive(Debug, Clone, Serialize)]
struct BundleManifest {
    #[serde(rename = "gameSlug")]
    game_slug: String,
    #[serde(rename = "gameEntry")]
    game_entry: String,
    #[serde(rename = "mathBaseUrl")]
    math_base_url: String,
    lang: String,
    currency: String,
    device: String,
}

/// Slugify a name for use as a folder + path segment (no spaces, no funky
/// punctuation). Strict ASCII subset.
fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Write the embedded preview-template + wasm-pkg + the user's bundle.json
/// plus game front + math into `dest_dir`. Idempotent — overwrites whatever is
/// there.
async fn assemble_bundle(
    dest_dir: &Path,
    profile: &Profile,
    front_path: &Path,
    math_mode: MathMode,
    app: Option<&AppHandle>,
    slug_label: &str,
) -> Result<u64> {
    fs::create_dir_all(dest_dir)
        .await
        .with_context(|| format!("create {}", dest_dir.display()))?;

    let mut total_bytes: u64 = 0;

    // 1) Static template (index.html, runtime.js, …). Skip the example
    // bundle so we don't ship it in production previews.
    for f in PREVIEW_TEMPLATE.files() {
        if f.path()
            .file_name()
            .is_some_and(|n| n == "bundle.example.json")
        {
            continue;
        }
        let out = dest_dir.join(f.path());
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).await.ok();
        }
        fs::write(&out, f.contents()).await?;
        total_bytes += f.contents().len() as u64;
    }

    // 2) WASM pkg next to the runtime so `import './lgs_wasm.js'` resolves.
    for f in WASM_PKG.files() {
        let name = match f.path().file_name() {
            Some(n) => n,
            None => continue,
        };
        // Skip wasm-pack metadata that doesn't ship.
        let n = name.to_string_lossy();
        if n == "package.json" || n == ".gitignore" || n.ends_with(".d.ts") {
            continue;
        }
        let out = dest_dir.join(name);
        fs::write(&out, f.contents()).await?;
        total_bytes += f.contents().len() as u64;
    }

    // 3) bundle.json — the runtime config picked up by runtime.js.
    let manifest = BundleManifest {
        game_slug: profile.game_slug.clone(),
        game_entry: "./game/index.html".to_string(),
        math_base_url: "./math".to_string(),
        lang: "en".to_string(),
        currency: "USD".to_string(),
        device: "desktop".to_string(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    fs::write(dest_dir.join("bundle.json"), &manifest_bytes).await?;
    total_bytes += manifest_bytes.len() as u64;

    // 4) Copy the game's built front into game/. Inject the preview shim
    // into game/index.html so it patches fetch + XHR before the game's own
    // scripts run.
    let game_dest = dest_dir.join("game");
    let copied = copy_dir(front_path, &game_dest).await?;
    total_bytes += copied;
    inject_shim_into_game_index(&game_dest).await.ok();

    // 5) Math files. In Full mode we copy them as-is; in Partial mode we
    // halve each book's `events` array — preview still plays but RTP /
    // animations are deliberately broken, mitigating the public-hosting
    // privacy hit.
    let math_dest = dest_dir.join("math");
    let math_src = PathBuf::from(&profile.game_path);
    let copied = match math_mode {
        MathMode::Full => copy_dir(&math_src, &math_dest).await?,
        MathMode::Partial => {
            copy_with_halved_events(&math_src, &math_dest, app, slug_label).await?
        }
        MathMode::Sampled => copy_with_sampled_math(&math_src, &math_dest, app, slug_label).await?,
    };
    total_bytes += copied;

    Ok(total_bytes)
}

/// Copy a math folder into `dst`, but rewrite every book listed in
/// `index.json` so its events array is truncated to its first half. Weights,
/// index.json and any other files are passed through unchanged. Mode names
/// are pulled from `index.json` so this works for any game.
async fn copy_with_halved_events(
    src: &Path,
    dst: &Path,
    app: Option<&AppHandle>,
    slug_label: &str,
) -> Result<u64> {
    fs::create_dir_all(dst).await?;
    let mut total: u64 = 0;

    let index_bytes = fs::read(src.join("index.json"))
        .await
        .context("read index.json")?;
    fs::write(dst.join("index.json"), &index_bytes).await?;
    total += index_bytes.len() as u64;

    let index: serde_json::Value =
        serde_json::from_slice(&index_bytes).context("parse index.json")?;
    let modes = index
        .get("modes")
        .and_then(|m| m.as_array())
        .ok_or_else(|| anyhow!("index.json: missing 'modes' array"))?;

    // Track the book + weights filenames the index references so we know
    // which files need transforming vs. straight copy. Use ordered Vec so
    // progress reports a stable file order.
    let mut book_files: Vec<String> = Vec::new();
    let mut weight_files: Vec<String> = Vec::new();
    for m in modes {
        if let Some(s) = m.get("events").and_then(|v| v.as_str())
            && !book_files.contains(&s.to_string())
        {
            book_files.push(s.to_string());
        }
        if let Some(s) = m.get("weights").and_then(|v| v.as_str())
            && !weight_files.contains(&s.to_string())
        {
            weight_files.push(s.to_string());
        }
    }

    // Pre-compute total compressed bytes so the progress bar has a real
    // denominator for the halving phase.
    let mut book_sizes: Vec<u64> = Vec::with_capacity(book_files.len());
    let mut bytes_total: u64 = 0;
    for f in &book_files {
        let meta = fs::metadata(src.join(f))
            .await
            .with_context(|| format!("stat {f}"))?;
        book_sizes.push(meta.len());
        bytes_total += meta.len();
    }

    let file_count = book_files.len() as u32;
    let mut bytes_done: u64 = 0;

    // Halve each book file referenced by the index. We thread a progress
    // closure into `halve_book_file` so the bar advances *during* one file
    // (decompress + truncate + recompress takes minutes for ~1 GB books).
    for (ix, book_rel) in book_files.iter().enumerate() {
        let in_path = src.join(book_rel);
        let out_path = dst.join(book_rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).await.ok();
        }

        let app_clone = app.cloned();
        let slug_clone = slug_label.to_string();
        let book_rel_clone = book_rel.clone();
        let file_size = book_sizes[ix];
        let bytes_done_at_start = bytes_done;
        let bytes_total_for_cb = bytes_total;
        let file_index = ix as u32;
        let progress_cb = move |dec_done: u64, dec_total: u64| {
            let Some(app) = app_clone.as_ref() else {
                return;
            };
            // Linearly scale per-file decompressed-bytes-done into the
            // file's share of the cross-file compressed-byte total.
            let frac = if dec_total == 0 {
                0.0
            } else {
                dec_done as f64 / dec_total as f64
            };
            let approx = (file_size as f64 * frac) as u64;
            emit_progress(
                app,
                &slug_clone,
                "hashing",
                &book_rel_clone,
                file_index,
                file_count,
                bytes_done_at_start + approx,
                bytes_total_for_cb,
            );
        };

        let halved = halve_book_file(&in_path, progress_cb)
            .await
            .with_context(|| format!("halve {}", book_rel))?;
        fs::write(&out_path, &halved).await?;
        total += halved.len() as u64;
        bytes_done += file_size;
    }

    // Copy weights verbatim. Tiny files — no progress needed.
    for w in &weight_files {
        let in_path = src.join(w);
        let bytes = fs::read(&in_path).await?;
        let out_path = dst.join(w);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).await.ok();
        }
        fs::write(&out_path, &bytes).await?;
        total += bytes.len() as u64;
    }

    Ok(total)
}

/// Read a `.jsonl.zst` book file, truncate every record's `events` array to
/// its first half, recompress. Returns the new compressed bytes.
///
/// `on_progress(dec_done, dec_total)` is called periodically with the
/// number of decompressed bytes consumed and the total — the caller scales
/// that into whatever progress space it wants.
///
/// The hot loop avoids `serde_json::Value` entirely: parsing + re-serializing
/// 1 M+ event objects per book file dominated wall-clock. We instead scan
/// each line byte-by-byte to locate the `"events":[ … ]` array, count its
/// top-level commas, and splice the buffer to drop the second half of the
/// items. Everything outside the array is copied verbatim. Roughly an order
/// of magnitude faster than the parse-then-serialize approach.
async fn halve_book_file<F>(path: &Path, on_progress: F) -> Result<Vec<u8>>
where
    F: Fn(u64, u64) + Send + Sync + 'static,
{
    let bytes = fs::read(path).await?;
    let halved = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let decompressed = zstd::decode_all(&bytes[..]).map_err(|e| anyhow!("zstd decode: {e}"))?;
        let total = decompressed.len() as u64;

        let mut out = Vec::with_capacity(decompressed.len() / 2);
        let mut done: u64 = 0;
        let mut last_emit: u64 = 0;
        let tick = (total / 50).max(1);

        let buf = &decompressed[..];
        let mut line_start = 0usize;
        let mut i = 0usize;
        while i <= buf.len() {
            let at_eof = i == buf.len();
            if at_eof || buf[i] == b'\n' {
                let line_end = i;
                if line_end > line_start {
                    let line = &buf[line_start..line_end];
                    write_halved_line(line, &mut out);
                    out.push(b'\n');
                }
                done += (line_end - line_start) as u64 + 1;
                line_start = i + 1;
                if at_eof {
                    break;
                }
                if done - last_emit >= tick {
                    on_progress(done, total);
                    last_emit = done;
                }
            }
            i += 1;
        }
        on_progress(done, total);

        // Level 3 + zstdmt: a few seconds for several GB instead of minutes.
        // Preview math doesn't need a max-ratio compression.
        let workers = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4)
            .min(8);
        let mut encoder = zstd::Encoder::new(Vec::with_capacity(out.len() / 2), 3)
            .map_err(|e| anyhow!("zstd encoder: {e}"))?;
        encoder
            .multithread(workers)
            .map_err(|e| anyhow!("zstd multithread({workers}): {e}"))?;
        std::io::Write::write_all(&mut encoder, &out).map_err(|e| anyhow!("zstd write: {e}"))?;
        let compressed = encoder.finish().map_err(|e| anyhow!("zstd finish: {e}"))?;
        Ok(compressed)
    })
    .await
    .map_err(|e| anyhow!("blocking task: {e}"))??;
    Ok(halved)
}

/// Write `line` to `out` with the `"events"` JSON array truncated to its
/// first half. Falls back to writing the line as-is if the structure
/// doesn't match expectations (no `"events":[…]`, malformed, etc.) — the
/// preview is best-effort, not a hard contract.
fn write_halved_line(line: &[u8], out: &mut Vec<u8>) {
    let Some(array_open) = find_events_array_open(line) else {
        out.extend_from_slice(line);
        return;
    };

    // Walk from `[` tracking JSON depth + in-string state. Record top-level
    // comma positions and the matching `]`.
    let mut depth = 1usize; // already inside the events array
    let mut in_string = false;
    let mut escape = false;
    let mut commas: Vec<usize> = Vec::new();
    let mut close: Option<usize> = None;
    let mut j = array_open + 1;
    while j < line.len() {
        let c = line[j];
        if escape {
            escape = false;
        } else if in_string {
            if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'[' | b'{' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                b'}' => {
                    depth = depth.saturating_sub(1);
                }
                b',' if depth == 1 => commas.push(j),
                _ => {}
            }
        }
        j += 1;
    }

    let Some(close_idx) = close else {
        out.extend_from_slice(line);
        return;
    };

    // item_count = commas + 1 if the array has any non-whitespace content;
    // 0 otherwise (empty `[]`).
    let inner_has_content = line[array_open + 1..close_idx]
        .iter()
        .any(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r'));
    let item_count = if inner_has_content {
        commas.len() + 1
    } else {
        0
    };
    let keep = item_count.div_ceil(2);

    if keep >= item_count {
        // Empty or single-item array — nothing to truncate.
        out.extend_from_slice(line);
        return;
    }

    // Truncate at the comma after the `keep`-th item (1-based), drop the
    // rest of the array, keep the closing `]` and everything past it.
    let truncate_at = commas[keep - 1];
    out.extend_from_slice(&line[..truncate_at]);
    out.push(b']');
    out.extend_from_slice(&line[close_idx + 1..]);
}

/// Copy a math folder into `dst`, but for every mode keep only ~100 books
/// covering a curated payout distribution (~50 no-wins + max + average +
/// quartile spread). Output files are tiny (a few MB total) so publishing
/// is fast and the preview plays end-to-end with reduced variety.
async fn copy_with_sampled_math(
    src: &Path,
    dst: &Path,
    app: Option<&AppHandle>,
    slug_label: &str,
) -> Result<u64> {
    fs::create_dir_all(dst).await?;
    let mut total: u64 = 0;

    let index_bytes = fs::read(src.join("index.json"))
        .await
        .context("read index.json")?;
    fs::write(dst.join("index.json"), &index_bytes).await?;
    total += index_bytes.len() as u64;

    let index: serde_json::Value =
        serde_json::from_slice(&index_bytes).context("parse index.json")?;
    let modes = index
        .get("modes")
        .and_then(|m| m.as_array())
        .ok_or_else(|| anyhow!("index.json: missing 'modes' array"))?;

    #[derive(Clone)]
    struct ModeRef {
        name: String,
        events: String,
        weights: String,
    }
    let mode_refs: Vec<ModeRef> = modes
        .iter()
        .filter_map(|m| {
            Some(ModeRef {
                name: m.get("name")?.as_str()?.to_string(),
                events: m.get("events")?.as_str()?.to_string(),
                weights: m.get("weights")?.as_str()?.to_string(),
            })
        })
        .collect();

    let total_modes = mode_refs.len() as u32;

    // Per-mode work runs in parallel: streaming zstd decode bounds memory at
    // a few MB regardless of input size, and modes are independent. Tokio's
    // blocking thread pool handles the CPU-bound zstd work without
    // saturating the runtime.
    let done_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let app_arc = app.cloned();
    let label_arc = std::sync::Arc::new(slug_label.to_string());

    let tasks = mode_refs.iter().map(|mode| {
        let src = src.to_path_buf();
        let dst = dst.to_path_buf();
        let mode = mode.clone();
        let done_count = done_count.clone();
        let app_arc = app_arc.clone();
        let label_arc = label_arc.clone();
        async move {
            // Sample weights upfront — small file, fast.
            let weights_path = src.join(&mode.weights);
            let weights_text = fs::read_to_string(&weights_path)
                .await
                .with_context(|| format!("read {}", mode.weights))?;
            let entries = parse_weights(&weights_text)?;
            let picked = sample_event_ids(&entries);
            let picked_set: std::collections::HashSet<u32> = picked.iter().copied().collect();

            let mut new_csv = String::new();
            for e in &entries {
                if picked_set.contains(&e.event_id) {
                    use std::fmt::Write;
                    let _ = writeln!(&mut new_csv, "{},1,{}", e.event_id, e.payout_multiplier);
                }
            }
            fs::write(dst.join(&mode.weights), new_csv.as_bytes()).await?;

            // Streaming filter on the books file — never materialise the
            // multi-GB decompressed buffer in memory.
            let books_path = src.join(&mode.events);
            let books_zst = fs::read(&books_path)
                .await
                .with_context(|| format!("read {}", mode.events))?;
            let mode_name = mode.name.clone();
            let books_out = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
                stream_filter_books(&books_zst, &picked_set)
                    .with_context(|| format!("filter books for mode '{mode_name}'"))
            })
            .await
            .map_err(|e| anyhow!("blocking task: {e}"))??;
            let books_out_len = books_out.len() as u64;
            fs::write(dst.join(&mode.events), &books_out).await?;

            // Progress: each completed mode bumps the counter. We use the
            // mode name as the current_file label so the bar stays
            // informative even when modes finish out of order.
            let done = done_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(app) = app_arc.as_ref() {
                emit_progress(
                    app,
                    &label_arc,
                    "hashing",
                    &mode.name,
                    done,
                    total_modes,
                    done as u64,
                    total_modes as u64,
                );
            }

            Ok::<u64, anyhow::Error>(new_csv.len() as u64 + books_out_len)
        }
    });

    let results: Vec<Result<u64>> = futures_util::future::join_all(tasks).await;
    for r in results {
        total += r?;
    }

    Ok(total)
}

/// Stream-decompress `books_zst` (a `.jsonl.zst`) and write only the lines
/// whose `id` field is in `picked` to a fresh zstd-compressed JSONL output.
/// Memory stays bounded at a few MB regardless of input size — critical
/// when the source is multi-GB compressed.
fn stream_filter_books(
    books_zst: &[u8],
    picked: &std::collections::HashSet<u32>,
) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut decoder =
        zstd::stream::read::Decoder::new(books_zst).map_err(|e| anyhow!("zstd decoder: {e}"))?;

    let mut out_lines = Vec::with_capacity(picked.len() * 2048);
    let mut line_buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = vec![0u8; 1024 * 1024];

    loop {
        let n = decoder
            .read(&mut chunk)
            .map_err(|e| anyhow!("zstd read: {e}"))?;
        if n == 0 {
            break;
        }
        line_buf.extend_from_slice(&chunk[..n]);
        while let Some(nl) = line_buf.iter().position(|&b| b == b'\n') {
            let line = &line_buf[..nl];
            if let Some(id) = extract_book_id(line)
                && picked.contains(&id)
            {
                out_lines.extend_from_slice(line);
                out_lines.push(b'\n');
            }
            line_buf.drain(..=nl);
        }
    }
    // Trailing line without `\n`.
    if !line_buf.is_empty()
        && let Some(id) = extract_book_id(&line_buf)
        && picked.contains(&id)
    {
        out_lines.extend_from_slice(&line_buf);
        out_lines.push(b'\n');
    }

    let workers = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4)
        .min(8);
    let mut encoder = zstd::Encoder::new(Vec::with_capacity(out_lines.len()), 3)
        .map_err(|e| anyhow!("zstd encoder: {e}"))?;
    encoder
        .multithread(workers)
        .map_err(|e| anyhow!("zstd multithread: {e}"))?;
    std::io::Write::write_all(&mut encoder, &out_lines).map_err(|e| anyhow!("zstd write: {e}"))?;
    encoder.finish().map_err(|e| anyhow!("zstd finish: {e}"))
}

/// Pick ~100 event ids from a mode's weight table with a curated payout
/// distribution: ~50 no-wins, the max-payout zone, the avg-winner, and
/// spread across quartiles of remaining winners.
fn sample_event_ids(entries: &[WeightEntry]) -> Vec<u32> {
    use rand::SeedableRng;
    use rand::seq::SliceRandom;
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xc0ffee);

    let no_wins: Vec<&WeightEntry> = entries
        .iter()
        .filter(|e| e.payout_multiplier == 0)
        .collect();
    let mut winners: Vec<&WeightEntry> =
        entries.iter().filter(|e| e.payout_multiplier > 0).collect();
    winners.sort_by_key(|e| e.payout_multiplier);

    let mut picks: Vec<u32> = Vec::new();

    // Up to 50 random no-wins.
    let mut nw = no_wins.clone();
    nw.shuffle(&mut rng);
    picks.extend(nw.iter().take(50).map(|e| e.event_id));

    if winners.is_empty() {
        return picks;
    }

    // Top 5 by multiplier — the "max win" zone.
    for e in winners.iter().rev().take(5) {
        picks.push(e.event_id);
    }

    // The "average winner" — closest to weight-weighted mean multiplier.
    let total_weight: u128 = winners.iter().map(|e| e.weight as u128).sum();
    let weighted_sum: u128 = winners
        .iter()
        .map(|e| e.weight as u128 * e.payout_multiplier as u128)
        .sum();
    let avg = (weighted_sum / total_weight.max(1)) as u32;
    if let Some(avg_entry) = winners
        .iter()
        .min_by_key(|e| e.payout_multiplier.abs_diff(avg))
    {
        picks.push(avg_entry.event_id);
    }

    // 7 random samples from each quartile of winners (low/mid/high).
    let n = winners.len();
    if n >= 4 {
        for q in [n / 4, n / 2, 3 * n / 4] {
            let lo = q.saturating_sub(8);
            let hi = (q + 8).min(n);
            let mut slice = winners[lo..hi].to_vec();
            slice.shuffle(&mut rng);
            picks.extend(slice.iter().take(7).map(|e| e.event_id));
        }
    } else {
        // Few winners: just take them all.
        picks.extend(winners.iter().map(|e| e.event_id));
    }

    picks.sort();
    picks.dedup();
    if picks.len() > 100 {
        picks.shuffle(&mut rng);
        picks.truncate(100);
    }
    picks
}

#[derive(Debug, Clone, Copy)]
struct WeightEntry {
    event_id: u32,
    weight: u64,
    payout_multiplier: u32,
}

fn parse_weights(text: &str) -> Result<Vec<WeightEntry>> {
    let mut entries = Vec::with_capacity(1024);
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split(',');
        let event_id = it
            .next()
            .ok_or_else(|| anyhow!("weights line {lineno}: missing eventId"))?
            .trim()
            .parse::<u32>()
            .with_context(|| format!("weights line {lineno}: eventId"))?;
        let weight = it
            .next()
            .ok_or_else(|| anyhow!("weights line {lineno}: missing weight"))?
            .trim()
            .parse::<u64>()
            .with_context(|| format!("weights line {lineno}: weight"))?;
        let payout_multiplier = it
            .next()
            .ok_or_else(|| anyhow!("weights line {lineno}: missing payout"))?
            .trim()
            .parse::<u32>()
            .with_context(|| format!("weights line {lineno}: payout"))?;
        entries.push(WeightEntry {
            event_id,
            weight,
            payout_multiplier,
        });
    }
    Ok(entries)
}

/// Pull the `id` value out of a book JSONL line without parsing the whole
/// thing. Same scan used by the LGS engine.
fn extract_book_id(line: &[u8]) -> Option<u32> {
    let i = skip_ws(line, 0);
    if *line.get(i)? != b'{' {
        return None;
    }
    let i = skip_ws(line, i + 1);
    if line.get(i..i + 4)? != b"\"id\"" {
        return None;
    }
    let i = skip_ws(line, i + 4);
    if *line.get(i)? != b':' {
        return None;
    }
    let start = skip_ws(line, i + 1);
    let mut end = start;
    while line.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == start {
        return None;
    }
    std::str::from_utf8(&line[start..end]).ok()?.parse().ok()
}

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    i
}

/// Locate the index of the `[` byte that opens the `"events"` array. Skips
/// over whitespace + `:` between the key and the array. Returns `None` if
/// the key isn't present or the syntax is unexpected.
fn find_events_array_open(line: &[u8]) -> Option<usize> {
    const KEY: &[u8] = b"\"events\"";
    let mut start = 0usize;
    while let Some(rel) = find_subseq(&line[start..], KEY) {
        let after = start + rel + KEY.len();
        let mut i = after;
        while i < line.len() && matches!(line[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i < line.len() && line[i] == b':' {
            i += 1;
            while i < line.len() && matches!(line[i], b' ' | b'\t' | b'\n' | b'\r') {
                i += 1;
            }
            if i < line.len() && line[i] == b'[' {
                return Some(i);
            }
        }
        start = after;
    }
    None
}

fn find_subseq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Insert `<script src="../preview-shim.js"></script>` immediately after
/// `<head>` in the game's index.html. The shim patches fetch + XHR so RGS
/// calls hit the WASM engine on the parent window instead of going to the
/// network. Runs *before* the game's bundle imports, which is the only way
/// to be certain the game uses the patched APIs.
async fn inject_shim_into_game_index(game_dest: &Path) -> Result<()> {
    let index = game_dest.join("index.html");
    if !index.exists() {
        return Ok(());
    }
    let html = fs::read_to_string(&index).await?;
    let tag = r#"<script src="../preview-shim.js"></script>"#;
    let lower = html.to_lowercase();
    let injected = if let Some(start) = lower.find("<head") {
        if let Some(rel_close) = lower[start..].find('>') {
            let pos = start + rel_close + 1;
            format!("{}{}{}", &html[..pos], tag, &html[pos..])
        } else {
            format!("{tag}{html}")
        }
    } else {
        format!("{tag}{html}")
    };
    fs::write(&index, injected).await?;
    Ok(())
}

async fn copy_dir(src: &Path, dst: &Path) -> Result<u64> {
    if !src.is_dir() {
        return Err(anyhow!("not a directory: {}", src.display()));
    }
    fs::create_dir_all(dst).await?;
    let mut total: u64 = 0;
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        let mut entries = fs::read_dir(&from).await?;
        while let Some(entry) = entries.next_entry().await? {
            let ft = entry.file_type().await?;
            let name = entry.file_name();
            let from_p = entry.path();
            let to_p = to.join(&name);
            if ft.is_dir() {
                fs::create_dir_all(&to_p).await?;
                stack.push((from_p, to_p));
            } else if ft.is_file() {
                let bytes = fs::read(&from_p).await?;
                total += bytes.len() as u64;
                fs::write(&to_p, &bytes).await?;
            }
        }
    }
    Ok(total)
}

/// Build a bundle directory for `profile` and return its absolute path. Used
/// by the "Preview locally" flow before/instead of publishing.
pub async fn build_local(
    profile_id: &str,
    front_path: String,
    math_mode: MathMode,
) -> Result<String> {
    let profile = crate::profiles::list()
        .await?
        .into_iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| anyhow!("profile not found"))?;
    let front = PathBuf::from(&front_path);
    if !front.is_dir() {
        return Err(anyhow!(
            "front path is not a directory: {}",
            front.display()
        ));
    }
    // Folder is keyed on profile.id (immutable UUID) rather than the user-
    // editable name so renaming a profile doesn't orphan the previous build.
    let dest = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("no local data dir"))?
        .join("stake-dev-tool")
        .join("previews")
        .join(&profile.id);
    // Wipe any previous build so we start fresh.
    let _ = fs::remove_dir_all(&dest).await;
    assemble_bundle(&dest, &profile, &front, math_mode, None, "").await?;
    Ok(dest.to_string_lossy().into_owned())
}

pub async fn publish_with_progress(
    app: &AppHandle,
    profile_id: &str,
    front_path: String,
    math_mode: MathMode,
) -> Result<PublishReport> {
    publish_inner(app, profile_id, front_path, math_mode).await
}

async fn publish_inner(
    app: &AppHandle,
    profile_id: &str,
    front_path: String,
    math_mode: MathMode,
) -> Result<PublishReport> {
    let profile = crate::profiles::list()
        .await?
        .into_iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| anyhow!("profile not found"))?;
    let front = PathBuf::from(&front_path);
    if !front.is_dir() {
        return Err(anyhow!(
            "front path is not a directory: {}",
            front.display()
        ));
    }

    // `preview_label` is purely cosmetic — it identifies the run in progress
    // events sent to the UI. The GitHub repo name is keyed on `profile.id`
    // so a rename can't orphan the public repo (which serves math).
    let preview_label = slug(&profile.name);

    emit_progress(
        app,
        &preview_label,
        "hashing",
        if math_mode == MathMode::Partial {
            "preparing math (halving)…"
        } else {
            "preparing math…"
        },
        0,
        0,
        0,
        0,
    );

    let staging = std::env::temp_dir()
        .join("stake-dev-tool-preview-build")
        .join(&profile.id);
    let _ = fs::remove_dir_all(&staging).await;
    assemble_bundle(
        &staging,
        &profile,
        &front,
        math_mode,
        Some(app),
        &preview_label,
    )
    .await?;

    let user = crate::github::auth::current_user()
        .await?
        .ok_or_else(|| anyhow!("not signed in to GitHub"))?;
    let client = GithubClient::from_stored_token()?;

    // Ensure a per-preview repo exists. One repo per preview means each
    // gets its own Pages 1 GB budget and a clean unpublish (`DELETE /repos`).
    let owner = user.login.clone();
    let repo_name = preview_repo_name(&profile.id);
    let repo = match client.get_repo(&owner, &repo_name).await {
        Ok(r) => r,
        Err(_) => {
            // Public — free GitHub Pages requires it.
            tracing::info!(repo = %repo_name, "creating preview repo for {owner}");
            create_public_repo(&client, &repo_name).await?
        }
    };

    // No URL prefix: each preview lives at the root of its own dedicated
    // repo (`stake-dev-tool-preview-<slug>`), so the Pages site root *is*
    // the preview root. Same-origin fetches → no CORS issues.
    let prefix = String::new();

    // Split big math files (anything over 80 MB) into chunks committed to
    // the repo as plain files. Pages serves them with the right same-origin
    // headers, sidestepping the Release-asset CORS dead-end. index.json
    // stays as-is so the WASM can bootstrap from a single small file.
    let math_dir = staging.join("math");
    let mut math_files: Vec<String> = Vec::new();
    collect_relative(&math_dir, &math_dir, &mut math_files).await?;
    math_files.retain(|p| p != "index.json");

    let chunks_dir = math_dir.join("chunks");
    let staged: Vec<file_chunker::StagedFile> = if math_files.is_empty() {
        Vec::new()
    } else {
        emit_progress(
            app,
            &preview_label,
            "hashing",
            "splitting math into chunks…",
            0,
            math_files.len() as u32,
            0,
            0,
        );
        file_chunker::split_to_dir(&math_dir, &chunks_dir, &math_files, "./math/chunks")
            .await
            .context("split math files into chunks")?
    };

    // The original .zst / .csv files have been chunked — remove them from
    // staging so we don't double-ship the data.
    for rel in &math_files {
        let _ = fs::remove_file(math_dir.join(rel)).await;
    }
    let manifest_json = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": 2,
        "files": staged,
    }))?;
    fs::write(math_dir.join("math-manifest.json"), &manifest_json).await?;

    // Walk the (now math-manifest-only) staging dir and PUT every file via
    // Contents API.
    let mut to_upload: Vec<(String, Vec<u8>)> = Vec::new();
    collect_files(&staging, &staging, &mut to_upload).await?;

    let bundle_total_bytes: u64 = to_upload.iter().map(|(_, b)| b.len() as u64).sum();
    let bundle_file_count = to_upload.len() as u32;

    // Get current branch head so we can build a new tree on top of it. The
    // default branch on a freshly-created repo is `main`.
    let head = client
        .get_branch_head(&owner, &repo.name, "main")
        .await
        .context("get main branch head")?;

    // Upload every file as a blob in parallel. Bounded concurrency keeps us
    // well under GitHub's per-second rate limit while saturating bandwidth.
    const PARALLEL_BLOBS: usize = 6;
    let bytes_uploaded_state = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let files_done_state = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

    let app_clone = app.clone();
    let label_clone = preview_label.clone();
    let total_bytes = bundle_total_bytes;
    let total_files = bundle_file_count;

    let owner_arc = owner.clone();
    let repo_name = repo.name.clone();
    let blobs_stream = stream::iter(to_upload)
        .map(|(rel, bytes)| {
            let client = client.clone();
            let owner = owner_arc.clone();
            let repo_name = repo_name.clone();
            let bytes_state = bytes_uploaded_state.clone();
            let files_state = files_done_state.clone();
            let app = app_clone.clone();
            let label = label_clone.clone();
            async move {
                let size = bytes.len() as u64;
                let sha = client
                    .create_blob(&owner, &repo_name, &bytes)
                    .await
                    .with_context(|| format!("blob {rel}"))?;
                let new_bytes =
                    bytes_state.fetch_add(size, std::sync::atomic::Ordering::Relaxed) + size;
                let new_files = files_state.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                emit_progress(
                    &app,
                    &label,
                    "uploading",
                    &rel,
                    new_files,
                    total_files,
                    new_bytes,
                    total_bytes,
                );
                Ok::<_, anyhow::Error>((rel, sha))
            }
        })
        .buffer_unordered(PARALLEL_BLOBS);

    // Bail loudly if any blob upload fails — silently dropping a file
    // would leave the manifest pointing at a missing chunk and the runtime
    // would 404. Better to fail the whole publish and let the user retry.
    let raw_results: Vec<Result<(String, String), anyhow::Error>> = blobs_stream.collect().await;
    let mut blob_results: Vec<(String, String)> = Vec::with_capacity(raw_results.len());
    for r in raw_results {
        match r {
            Ok(v) => blob_results.push(v),
            Err(e) => return Err(e).context("blob upload failed"),
        }
    }

    let files_uploaded = blob_results.len() as u32;
    let files_skipped = bundle_file_count - files_uploaded;
    let bytes_uploaded = bytes_uploaded_state.load(std::sync::atomic::Ordering::Relaxed);

    emit_progress(
        app,
        &preview_label,
        "committing",
        "creating tree + commit…",
        files_uploaded,
        bundle_file_count,
        bytes_uploaded,
        bundle_total_bytes,
    );

    // Stitch the blobs into a single tree on top of the existing one, then
    // a single commit, then move the branch ref. Same shape as `git push`
    // with a single commit — way faster than the Contents API's
    // one-commit-per-file pattern.
    let entries: Vec<GitTreeEntry> = blob_results
        .into_iter()
        .map(|(rel, sha)| GitTreeEntry::blob(format!("{prefix}{rel}"), sha))
        .collect();
    let new_tree = client
        .create_tree(&owner, &repo.name, &head.tree_sha, &entries)
        .await
        .context("create tree")?;
    let commit_msg = format!(
        "preview: publish {} ({} files)",
        profile.name, files_uploaded
    );
    let new_commit = client
        .create_commit(
            &owner,
            &repo.name,
            &commit_msg,
            &new_tree,
            &[&head.commit_sha],
        )
        .await
        .context("create commit")?;
    client
        .update_ref(&owner, &repo.name, "main", &new_commit)
        .await
        .context("update main ref")?;

    emit_progress(
        app,
        &preview_label,
        "committing",
        "enabling GitHub Pages…",
        bundle_file_count,
        bundle_file_count,
        bytes_uploaded,
        bundle_total_bytes,
    );

    // Enable Pages (idempotent — 409/422 already-enabled cases are mapped to
    // Ok inside `enable_pages`, so anything reaching the Err branch here is a
    // real failure: missing scope, transient 5xx, etc. Surfacing it stops us
    // from handing the user a "done" toast for a URL that will 404 forever.
    enable_pages(&client, &owner, &repo.name)
        .await
        .context("enable GitHub Pages")?;

    let url = format!("https://{}.github.io/{}/", owner.to_lowercase(), repo_name);

    // Pages can take 10 sec - 2 min to build + propagate after the API
    // call returns. Polling the URL until we get a 200 means the toast
    // doesn't shout "done" while the link is still 404.
    wait_for_pages(
        app,
        &preview_label,
        &url,
        bundle_file_count,
        bytes_uploaded,
        bundle_total_bytes,
    )
    .await;

    emit_progress(
        app,
        &preview_label,
        "done",
        "",
        bundle_file_count,
        bundle_file_count,
        bytes_uploaded,
        bundle_total_bytes,
    );

    Ok(PublishReport {
        url,
        files_uploaded,
        files_skipped,
        bytes_uploaded,
    })
}

/// Poll the preview URL until it serves 200 (deploy complete) or a 5 min
/// budget runs out. We don't fail on timeout — the URL is still returned;
/// the Pages build might just need a couple more minutes.
async fn wait_for_pages(
    app: &AppHandle,
    label: &str,
    url: &str,
    file_count: u32,
    bytes_done: u64,
    bytes_total: u64,
) {
    let client = match reqwest::Client::builder()
        .user_agent("stake-dev-tool")
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let start = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(60);
    loop {
        let elapsed = start.elapsed();
        if elapsed > budget {
            tracing::warn!(url, "pages deploy still 404 after 60s — giving up the wait");
            return;
        }
        emit_progress(
            app,
            label,
            "committing",
            &format!("deploying GitHub Pages… ({}s)", elapsed.as_secs()),
            file_count,
            file_count,
            bytes_done,
            bytes_total,
        );
        match client.get(url).send().await {
            Ok(r) if r.status().is_success() => return,
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Drop a preview by deleting its folder from the preview repo. The repo
/// itself stays.
pub async fn unpublish(profile_id: &str) -> Result<()> {
    let profile = crate::profiles::list()
        .await?
        .into_iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| anyhow!("profile not found"))?;
    let user = crate::github::auth::current_user()
        .await?
        .ok_or_else(|| anyhow!("not signed in"))?;
    let client = GithubClient::from_stored_token()?;
    // Match `publish_inner`: repo is keyed on the immutable profile id, NOT
    // on the user-editable name (otherwise a rename would point unpublish at
    // a non-existent repo and silently leave the original public).
    let repo_name = preview_repo_name(&profile.id);

    // One repo per preview → unpublish is just `DELETE /repos`. Same scope
    // as the OAuth `delete_repo` we already grant.
    client
        .delete_repo(&user.login, &repo_name)
        .await
        .with_context(|| format!("delete preview repo {}/{}", user.login, repo_name))?;
    Ok(())
}

async fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    let mut entries = fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let ft = entry.file_type().await?;
        let path = entry.path();
        if ft.is_dir() {
            Box::pin(collect_files(root, &path, out)).await?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path).await?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

async fn collect_relative(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let ft = entry.file_type().await?;
        let path = entry.path();
        if ft.is_dir() {
            Box::pin(collect_relative(root, &path, out)).await?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

async fn create_public_repo(
    client: &GithubClient,
    name: &str,
) -> Result<crate::github::api::RepoInfo> {
    // The existing helper creates private repos; we need public for free
    // GitHub Pages. Inline the call here.
    let url = "https://api.github.com/user/repos";
    let token = crate::github::auth::load_token()?.ok_or_else(|| anyhow!("no token"))?;
    let res = reqwest::Client::builder()
        .user_agent("stake-dev-tool")
        .build()?
        .post(url)
        .bearer_auth(&token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&serde_json::json!({
            "name": name,
            "description": "Public preview hosting for stake-dev-tool. Auto-generated.",
            "private": false,
            "auto_init": true,
            "has_issues": false,
            "has_projects": false,
            "has_wiki": false
        }))
        .send()
        .await?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(anyhow!("create preview repo: {status} {body}"));
    }
    let r: crate::github::api::RepoInfo = res.json().await?;

    // Wait until the contents endpoint is ready (auto_init is async on
    // GitHub's side).
    for _ in 0..6 {
        if client
            .get_file(&r.owner.login, &r.name, "README.md")
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let _ = client;
    Ok(r)
}

async fn enable_pages(_client: &GithubClient, owner: &str, repo: &str) -> Result<()> {
    let token = crate::github::auth::load_token()?.ok_or_else(|| anyhow!("no token"))?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/pages");
    let res = reqwest::Client::builder()
        .user_agent("stake-dev-tool")
        .build()?
        .post(&url)
        .bearer_auth(&token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&serde_json::json!({
            "source": { "branch": "main", "path": "/" }
        }))
        .send()
        .await?;
    let status = res.status();
    if status.is_success()
        || status == reqwest::StatusCode::CONFLICT
        || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
    {
        // 409/422 = already enabled.
        return Ok(());
    }
    let body = res.text().await.unwrap_or_default();
    Err(anyhow!("enable pages: {status} {body}"))
}
