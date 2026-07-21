//! The `sdt pull` command: download a revision's files to a directory.
//!
//! Fetches the revision detail, then streams each file straight to disk while
//! verifying its sha256, with per-file progress like `push`. The core
//! [`pull_files`] is generic over [`PlatformApi`] and reused by the `mcp`
//! server's `pull_revision` tool with a quiet reporter.

use std::path::{Path, PathBuf};

use crate::PullArgs;
use crate::api::{ApiClient, FileDownload, PlatformApi, resolve_head};
use crate::error::CliError;
use crate::output::{Reporter, Transfer};

/// Downloads every file of revision `number` into `dest`, returning the list of
/// relative paths written. Refuses to write into a non-empty directory unless
/// `force`. Verifies each file's sha256 as it streams (in the client).
pub async fn pull_files<C: PlatformApi>(
    client: &C,
    ws: &str,
    game: &str,
    number: i64,
    dest: &Path,
    reporter: &Reporter,
    force: bool,
) -> Result<Vec<String>, CliError> {
    let detail = client.get_revision(ws, game, number).await?;

    guard_destination(dest, force)?;
    std::fs::create_dir_all(dest)
        .map_err(|e| CliError::usage(anyhow_io(format!("creating {}: {e}", dest.display()))))?;

    let mut pulled = Vec::with_capacity(detail.files.len());
    for file in &detail.files {
        let out_path = safe_join(dest, &file.path)?;
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CliError::usage(anyhow_io(format!("creating {}: {e}", parent.display())))
            })?;
        }

        let task = reporter.start_file(&file.path, file.size, Transfer::Download);
        let progress = task.progress();
        let spec = FileDownload {
            ws,
            game,
            number,
            remote_path: &file.path,
            dest: out_path,
            expected_hash: &file.hash,
        };
        match client.download_file(&spec, progress).await {
            Ok(()) => {
                task.finish_success(file.size);
                pulled.push(file.path.clone());
            }
            Err(e) => {
                task.finish_error(&e.to_string());
                return Err(e.into());
            }
        }
    }
    Ok(pulled)
}

/// Entry point for the `pull` subcommand.
pub async fn run(client: &ApiClient, args: PullArgs) -> Result<(), CliError> {
    let number = match args.rev {
        Some(n) => n,
        None => resolve_head(client, &args.workspace, &args.game).await?,
    };
    let dest = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}-rev{}", args.game, number)));

    let reporter = Reporter::new(false);
    reporter.println(&format!("Pulling revision #{number} → {}", dest.display()));
    let files = pull_files(
        client,
        &args.workspace,
        &args.game,
        number,
        &dest,
        &reporter,
        args.force,
    )
    .await?;
    reporter.println(&format!(
        "Pulled {} file(s) into {}",
        files.len(),
        dest.display()
    ));

    // The written directory is the machine-usable result → stdout.
    println!("{}", dest.display());
    Ok(())
}

/// Errors unless `dest` is absent, an empty directory, or `force` is set.
fn guard_destination(dest: &Path, force: bool) -> Result<(), CliError> {
    if !dest.exists() {
        return Ok(());
    }
    if !dest.is_dir() {
        return Err(CliError::usage_msg(format!(
            "destination {} exists and is not a directory",
            dest.display()
        )));
    }
    let non_empty = std::fs::read_dir(dest)
        .map_err(|e| CliError::usage(anyhow_io(format!("reading {}: {e}", dest.display()))))?
        .next()
        .is_some();
    if non_empty && !force {
        return Err(CliError::usage_msg(format!(
            "destination {} is not empty; pass --force to overwrite",
            dest.display()
        )));
    }
    Ok(())
}

/// Joins a forward-slashed relative path onto `dest`, rejecting any traversal.
fn safe_join(dest: &Path, rel: &str) -> Result<PathBuf, CliError> {
    let mut out = dest.to_path_buf();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(CliError::usage_msg(format!(
                "refusing to write outside the destination: {rel}"
            )));
        }
        out.push(seg);
    }
    Ok(out)
}

fn anyhow_io(msg: String) -> anyhow::Error {
    anyhow::anyhow!("{msg}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_traversal() {
        let dest = Path::new("/out");
        assert!(safe_join(dest, "../escape").is_err());
        assert!(safe_join(dest, "a/../../b").is_err());
    }

    #[test]
    fn safe_join_builds_nested_paths() {
        let dest = Path::new("out");
        let joined = safe_join(dest, "nested/books.jsonl").unwrap();
        assert_eq!(joined, Path::new("out").join("nested").join("books.jsonl"));
    }

    #[test]
    fn guard_allows_absent_and_empty_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("does-not-exist");
        assert!(guard_destination(&absent, false).is_ok());
        // The tempdir itself is empty.
        assert!(guard_destination(dir.path(), false).is_ok());
    }

    #[test]
    fn guard_refuses_non_empty_without_force() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), b"x").unwrap();
        assert!(guard_destination(dir.path(), false).is_err());
        assert!(guard_destination(dir.path(), true).is_ok());
    }
}
