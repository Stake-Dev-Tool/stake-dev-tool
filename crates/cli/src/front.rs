//! The `sdt push-front` command: validate a front-bundle folder (a web build),
//! hash it, upload only the blobs the server is missing, then commit a bundle.
//!
//! Front bundles reuse the M2 blob machinery verbatim — the same
//! content-addressed store and the same `check → upload → commit` shape as math
//! revisions — so this mirrors [`crate::push`] closely, reusing its hashing and
//! upload pipeline. The network-touching core, [`execute_front_push`], is
//! generic over [`FrontBundleApi`] so its branchy logic (dedup, the
//! missing_blobs retry) is exercised by unit tests with a fake client.

use anyhow::anyhow;

use crate::PushFrontArgs;
use crate::api::{ApiClient, ClientError, FileEntry, FrontBundleApi};
use crate::error::CliError;
use crate::hash::{self, HashedFile};
use crate::output::{self, Reporter};
use crate::push;

/// What a front-bundle push produced, for the recap (and the `mcp` `push_front`
/// result).
#[derive(Debug)]
pub struct FrontOutcome {
    pub id: String,
    pub created_at: Option<String>,
    pub total_files: usize,
    pub total_bytes: u64,
    pub uploaded_count: usize,
    pub uploaded_bytes: u64,
}

/// Scan → hash → check → upload only missing blobs → commit the bundle. Shared
/// by the `push-front` command (progress reporter) and the `mcp` server's
/// `push_front` tool (quiet reporter). Generic over [`FrontBundleApi`] so it is
/// testable offline.
pub async fn push_front_folder<C: FrontBundleApi>(
    client: &C,
    path: &std::path::Path,
    ws: &str,
    game: &str,
    reporter: &Reporter,
) -> Result<FrontOutcome, CliError> {
    // Validate the folder (index.html at root, <= 2000 files) and build a
    // deterministic manifest.
    let entries = hash::scan_front_manifest(path).map_err(CliError::usage)?;
    if entries.is_empty() {
        return Err(CliError::usage_msg(
            "nothing to push: the bundle has no files",
        ));
    }

    let hashed = push::hash_all(entries, reporter)?;
    reporter.println(&output::front_summary(&hashed));

    let files: Vec<FileEntry> = hashed
        .iter()
        .map(|h| FileEntry {
            path: h.rel_path.clone(),
            hash: h.hash.clone(),
            size: h.size,
        })
        .collect();

    execute_front_push(client, ws, game, &hashed, &files, reporter).await
}

/// Entry point for the `push-front` subcommand.
pub async fn run(client: &ApiClient, args: PushFrontArgs) -> Result<(), CliError> {
    let reporter = Reporter::new(args.no_progress);
    let outcome =
        push_front_folder(client, &args.path, &args.workspace, &args.game, &reporter).await?;

    if args.json {
        // CI-facing: a machine-readable recap on stdout, nothing else.
        let json = serde_json::json!({
            "id": outcome.id,
            "created_at": outcome.created_at,
            "uploaded": outcome.uploaded_count,
            "total_files": outcome.total_files,
            "uploaded_bytes": outcome.uploaded_bytes,
            "deduplicated_bytes": outcome.total_bytes.saturating_sub(outcome.uploaded_bytes),
        });
        let text = serde_json::to_string_pretty(&json)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{text}");
    } else {
        // The bundle id is the machine-usable result → stdout.
        println!("{}", outcome.id);
        reporter.println(&output::front_recap(
            &outcome.id,
            outcome.total_files,
            outcome.uploaded_count,
            outcome.uploaded_bytes,
            outcome.total_bytes,
        ));
    }

    Ok(())
}

/// check → upload missing → commit, with the documented `missing_blobs` retry.
async fn execute_front_push<C: FrontBundleApi>(
    client: &C,
    ws: &str,
    game: &str,
    hashed: &[HashedFile],
    files: &[FileEntry],
    reporter: &Reporter,
) -> Result<FrontOutcome, CliError> {
    let total_bytes: u64 = hashed.iter().map(|h| h.size).sum();

    // Ask the server which blobs it lacks; upload only those.
    let missing = client.check_front_bundle(ws, game, files).await?;
    let to_upload = push::select_uploads(hashed, &missing);

    let mut uploaded_count = 0usize;
    let mut uploaded_bytes = 0u64;
    if to_upload.is_empty() {
        reporter.println("All blobs already present on the server; nothing to upload.");
    } else {
        reporter.println(&format!(
            "Uploading {} of {} file(s) not yet on the server…",
            to_upload.len(),
            hashed.len()
        ));
        uploaded_bytes += push::upload_missing(client, ws, game, &to_upload, reporter).await?;
        uploaded_count += to_upload.len();
    }

    let created = match client.create_front_bundle(ws, game, files).await {
        Ok(created) => created,
        // Race: a blob the server acked at check time is gone. Upload the listed
        // hashes and retry the commit exactly once.
        Err(ClientError::Api(api)) if api.code == "missing_blobs" => {
            let retry = push::select_uploads(hashed, &api.missing);
            if !retry.is_empty() {
                reporter.println(&format!(
                    "Server still needs {} blob(s); re-uploading and retrying the commit…",
                    retry.len()
                ));
                uploaded_bytes += push::upload_missing(client, ws, game, &retry, reporter).await?;
                uploaded_count += retry.len();
            }
            client.create_front_bundle(ws, game, files).await?
        }
        Err(e) => return Err(e.into()),
    };

    Ok(FrontOutcome {
        id: created.id,
        created_at: created.created_at,
        total_files: hashed.len(),
        total_bytes,
        uploaded_count,
        uploaded_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{
        ApiError, BlobUpload, ClientResult, CreateRevisionRequest, FrontBundleCreated, RevisionApi,
        RevisionDetail,
    };
    use crate::output::FileProgress;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Scripted `create_front_bundle` outcome.
    enum FakeCommit {
        Ok(String),
        MissingBlobs(Vec<String>),
    }

    /// In-memory [`FrontBundleApi`] that records uploads and replays scripted
    /// commit responses — no network, no disk.
    struct FakeFront {
        missing_on_check: Vec<String>,
        uploaded: Mutex<Vec<String>>,
        commit_calls: Mutex<u32>,
        commit_responses: Mutex<VecDeque<FakeCommit>>,
    }

    impl FakeFront {
        fn new(missing_on_check: Vec<String>, commits: Vec<FakeCommit>) -> Self {
            Self {
                missing_on_check,
                uploaded: Mutex::new(Vec::new()),
                commit_calls: Mutex::new(0),
                commit_responses: Mutex::new(commits.into()),
            }
        }
    }

    // Only `upload_blob` is exercised from `RevisionApi`; the rest are unused
    // here but required to satisfy the supertrait bound.
    impl RevisionApi for FakeFront {
        async fn check_files(
            &self,
            _ws: &str,
            _game: &str,
            _files: &[FileEntry],
        ) -> ClientResult<Vec<String>> {
            Ok(vec![])
        }
        async fn upload_blob(
            &self,
            _ws: &str,
            _game: &str,
            upload: &BlobUpload,
            _progress: FileProgress,
        ) -> ClientResult<()> {
            self.uploaded.lock().unwrap().push(upload.hash.clone());
            Ok(())
        }
        async fn create_revision(
            &self,
            _ws: &str,
            _game: &str,
            _req: &CreateRevisionRequest,
        ) -> ClientResult<RevisionDetail> {
            unreachable!("front push never creates a revision")
        }
        async fn get_revision(
            &self,
            _ws: &str,
            _game: &str,
            _number: i64,
        ) -> ClientResult<RevisionDetail> {
            unreachable!("front push never reads a revision")
        }
    }

    impl FrontBundleApi for FakeFront {
        async fn check_front_bundle(
            &self,
            _ws: &str,
            _game: &str,
            _files: &[FileEntry],
        ) -> ClientResult<Vec<String>> {
            Ok(self.missing_on_check.clone())
        }
        async fn create_front_bundle(
            &self,
            _ws: &str,
            _game: &str,
            _files: &[FileEntry],
        ) -> ClientResult<FrontBundleCreated> {
            *self.commit_calls.lock().unwrap() += 1;
            match self
                .commit_responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra create_front_bundle call")
            {
                FakeCommit::Ok(id) => Ok(FrontBundleCreated {
                    id,
                    created_at: None,
                    extra: Default::default(),
                }),
                FakeCommit::MissingBlobs(missing) => Err(ClientError::Api(ApiError {
                    status: 409,
                    code: "missing_blobs".into(),
                    message: "blobs missing".into(),
                    missing,
                })),
            }
        }
    }

    fn hf(rel: &str, hash: &str, size: u64) -> HashedFile {
        HashedFile {
            rel_path: rel.into(),
            path: PathBuf::from(rel),
            size,
            hash: hash.into(),
        }
    }

    fn files_of(hashed: &[HashedFile]) -> Vec<FileEntry> {
        hashed
            .iter()
            .map(|h| FileEntry {
                path: h.rel_path.clone(),
                hash: h.hash.clone(),
                size: h.size,
            })
            .collect()
    }

    #[tokio::test]
    async fn uploads_only_missing_blobs_then_commits() {
        let hashed = vec![
            hf("index.html", "h1", 10),
            hf("assets/app.js", "h2", 20),
            hf("assets/style.css", "h3", 30),
        ];
        let files = files_of(&hashed);
        let fake = FakeFront::new(vec!["h2".into()], vec![FakeCommit::Ok("bundle-9".into())]);
        let reporter = Reporter::new(true);

        let out = execute_front_push(&fake, "w", "g", &hashed, &files, &reporter)
            .await
            .unwrap();

        assert_eq!(out.id, "bundle-9");
        assert_eq!(out.uploaded_count, 1);
        assert_eq!(out.uploaded_bytes, 20);
        assert_eq!(out.total_bytes, 60); // dedup saved 40
        assert_eq!(*fake.uploaded.lock().unwrap(), vec!["h2".to_string()]);
    }

    #[tokio::test]
    async fn retries_commit_once_on_missing_blobs() {
        let hashed = vec![hf("index.html", "h1", 10), hf("assets/app.js", "h2", 20)];
        let files = files_of(&hashed);
        // Nothing missing at check time, but the first commit reports h2 gone.
        let fake = FakeFront::new(
            vec![],
            vec![
                FakeCommit::MissingBlobs(vec!["h2".into()]),
                FakeCommit::Ok("bundle-2".into()),
            ],
        );
        let reporter = Reporter::new(true);

        let out = execute_front_push(&fake, "w", "g", &hashed, &files, &reporter)
            .await
            .unwrap();

        assert_eq!(out.id, "bundle-2");
        assert_eq!(*fake.commit_calls.lock().unwrap(), 2); // retried exactly once
        assert_eq!(*fake.uploaded.lock().unwrap(), vec!["h2".to_string()]);
        assert_eq!(out.uploaded_count, 1);
    }
}
