//! The `sdt push` command: validate a math folder, hash it, upload only the
//! blobs the server is missing, then commit a revision.
//!
//! The network-touching core, [`execute_push`], is generic over
//! [`RevisionApi`] so its branchy logic (dedup, the missing_blobs retry, the
//! stale_parent abort) is exercised by unit tests with a fake client.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::anyhow;
use futures_util::{StreamExt, stream};
use tokio::time::{Instant, sleep};

use crate::PushArgs;
use crate::api::{
    ApiClient, BlobUpload, ClientError, ClientResult, CreateRevisionRequest, FileEntry,
    RevisionApi, RevisionDetail,
};
use crate::error::CliError;
use crate::hash::{self, HashedFile, ManifestEntry};
use crate::output::{self, FileProgress, Reporter, Transfer};

/// How many blob uploads run at once. Enough to fill the pipe on a fast
/// connection without opening an unbounded number of files or sockets.
const UPLOAD_CONCURRENCY: usize = 4;

/// Total upload attempts per blob: 1 initial + 3 retries (1s/2s/4s backoff).
const MAX_UPLOAD_ATTEMPTS: u32 = 4;

/// How often `--wait-stats` polls the revision detail.
const STATS_POLL_INTERVAL: Duration = Duration::from_secs(3);

/// The immutable inputs to a single push, bundled to keep [`execute_push`]
/// under the argument-count limit and to make the test call sites tidy.
struct PushJob<'a> {
    ws: &'a str,
    game: &'a str,
    message: &'a str,
    parent: Option<i64>,
    hashed: &'a [HashedFile],
    files: &'a [FileEntry],
}

/// What a push produced, for the recap (and the `mcp` `push_math` result).
#[derive(Debug)]
pub struct PushOutcome {
    pub detail: RevisionDetail,
    pub total_files: usize,
    pub total_bytes: u64,
    pub uploaded_count: usize,
    pub uploaded_bytes: u64,
}

/// Scan → hash → check → upload only missing blobs → commit. Shared by the
/// `push` command (progress reporter) and the `mcp` server's `push_math` tool
/// (quiet reporter). Generic over [`RevisionApi`] so it is testable offline.
pub async fn push_folder<C: RevisionApi>(
    client: &C,
    path: &std::path::Path,
    ws: &str,
    game: &str,
    message: &str,
    parent: Option<i64>,
    reporter: &Reporter,
) -> Result<PushOutcome, CliError> {
    // Validate the folder and build a deterministic manifest.
    let entries = hash::scan_manifest(path).map_err(CliError::usage)?;
    if entries.is_empty() {
        return Err(CliError::usage_msg(
            "nothing to push: the folder has no files",
        ));
    }

    let hashed = hash_all(entries, reporter)?;
    reporter.println(&output::push_summary(&hashed));

    let files: Vec<FileEntry> = hashed
        .iter()
        .map(|h| FileEntry {
            path: h.rel_path.clone(),
            hash: h.hash.clone(),
            size: h.size,
        })
        .collect();
    let job = PushJob {
        ws,
        game,
        message,
        parent,
        hashed: &hashed,
        files: &files,
    };

    execute_push(client, &job, reporter).await
}

/// Entry point for the `push` subcommand.
pub async fn run(client: &ApiClient, args: PushArgs) -> Result<(), CliError> {
    let reporter = Reporter::new(args.no_progress);
    let PushOutcome {
        detail: initial,
        total_files,
        total_bytes,
        uploaded_count,
        uploaded_bytes,
    } = push_folder(
        client,
        &args.path,
        &args.workspace,
        &args.game,
        &args.message,
        args.parent,
        &reporter,
    )
    .await?;

    // Optionally wait for the server to compute bet-stats.
    let detail = if args.wait_stats {
        wait_for_stats(
            client,
            &args.workspace,
            &args.game,
            initial.number,
            args.timeout,
            &reporter,
        )
        .await?
    } else {
        initial
    };

    if args.json {
        // CI-facing: the full response on stdout, nothing else.
        let json = serde_json::to_string_pretty(&detail)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
    } else {
        // The revision number is the machine-usable result → stdout.
        println!("{}", detail.number);
        reporter.println(&output::push_recap(
            detail.number,
            total_files,
            uploaded_count,
            uploaded_bytes,
            total_bytes,
        ));
        if args.wait_stats {
            let pending = detail
                .stats
                .as_ref()
                .map(|s| s.status == "pending")
                .unwrap_or(true);
            if pending {
                reporter.println(&format!(
                    "Bet-stats still pending after {}s; run `sdt revisions` later.",
                    args.timeout
                ));
            } else {
                reporter.println(&output::stats_table(&detail));
            }
        }
    }

    Ok(())
}

/// check → upload missing → commit, with the two documented 409 behaviours.
async fn execute_push<C: RevisionApi>(
    client: &C,
    job: &PushJob<'_>,
    reporter: &Reporter,
) -> Result<PushOutcome, CliError> {
    let total_bytes: u64 = job.hashed.iter().map(|h| h.size).sum();

    // Ask the server which blobs it lacks; upload only those.
    let missing = client.check_files(job.ws, job.game, job.files).await?;
    let to_upload = select_uploads(job.hashed, &missing);

    let mut uploaded_count = 0usize;
    let mut uploaded_bytes = 0u64;
    if to_upload.is_empty() {
        reporter.println("All blobs already present on the server; nothing to upload.");
    } else {
        reporter.println(&format!(
            "Uploading {} of {} file(s) not yet on the server…",
            to_upload.len(),
            job.hashed.len()
        ));
        uploaded_bytes += upload_missing(client, job.ws, job.game, &to_upload, reporter).await?;
        uploaded_count += to_upload.len();
    }

    let req = CreateRevisionRequest {
        message: job.message.to_string(),
        files: job.files.to_vec(),
        parent_number: job.parent,
    };

    let detail = match client.create_revision(job.ws, job.game, &req).await {
        Ok(detail) => detail,
        // Race: a blob the server acked at check time is gone. Upload the
        // listed hashes and retry the commit exactly once.
        Err(ClientError::Api(api)) if api.code == "missing_blobs" => {
            let retry = select_uploads(job.hashed, &api.missing);
            if !retry.is_empty() {
                reporter.println(&format!(
                    "Server still needs {} blob(s); re-uploading and retrying the commit…",
                    retry.len()
                ));
                uploaded_bytes +=
                    upload_missing(client, job.ws, job.game, &retry, reporter).await?;
                uploaded_count += retry.len();
            }
            client.create_revision(job.ws, job.game, &req).await?
        }
        // Someone pushed a newer revision than the --parent we targeted.
        Err(ClientError::Api(api)) if api.code == "stale_parent" => {
            return Err(CliError::usage_msg(format!(
                "stale parent: {} — a newer revision exists; omit --parent to target the latest, or re-pull and retry",
                api.message
            )));
        }
        Err(e) => return Err(e.into()),
    };

    Ok(PushOutcome {
        detail,
        total_files: job.hashed.len(),
        total_bytes,
        uploaded_count,
        uploaded_bytes,
    })
}

/// Picks the hashed files whose hash appears in `hashes`.
fn select_uploads(hashed: &[HashedFile], hashes: &[String]) -> Vec<BlobUpload> {
    let wanted: HashSet<&str> = hashes.iter().map(String::as_str).collect();
    hashed
        .iter()
        .filter(|h| wanted.contains(h.hash.as_str()))
        .map(|h| BlobUpload {
            hash: h.hash.clone(),
            path: h.path.clone(),
            size: h.size,
            rel_path: h.rel_path.clone(),
        })
        .collect()
}

/// Uploads blobs with bounded concurrency, returning the total bytes sent.
/// The first failure (after per-blob retries) aborts the whole push.
async fn upload_missing<C: RevisionApi>(
    client: &C,
    ws: &str,
    game: &str,
    uploads: &[BlobUpload],
    reporter: &Reporter,
) -> Result<u64, CliError> {
    let results: Vec<ClientResult<u64>> = stream::iter(uploads.iter())
        .map(|upload| async move {
            let task = reporter.start_file(&upload.rel_path, upload.size, Transfer::Upload);
            let progress = task.progress();
            let outcome = upload_one(client, ws, game, upload, &progress).await;
            match &outcome {
                Ok(()) => task.finish_success(upload.size),
                Err(e) => task.finish_error(&e.to_string()),
            }
            outcome.map(|()| upload.size)
        })
        .buffer_unordered(UPLOAD_CONCURRENCY)
        .collect()
        .await;

    let mut bytes = 0u64;
    for result in results {
        bytes += result?;
    }
    Ok(bytes)
}

/// Uploads one blob, retrying retryable failures up to [`MAX_UPLOAD_ATTEMPTS`].
async fn upload_one<C: RevisionApi>(
    client: &C,
    ws: &str,
    game: &str,
    upload: &BlobUpload,
    progress: &FileProgress,
) -> ClientResult<()> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        progress.reset();
        match client.upload_blob(ws, game, upload, progress.clone()).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if attempt >= MAX_UPLOAD_ATTEMPTS || !e.is_retryable() {
                    return Err(e);
                }
                // 1s, 2s, 4s.
                sleep(Duration::from_secs(1u64 << (attempt - 1))).await;
            }
        }
    }
}

/// Polls the revision until stats stop being `pending` or the timeout elapses.
/// On timeout the (still pending) detail is returned for the caller to note.
/// Shared with the `stats --wait` command.
pub(crate) async fn wait_for_stats(
    client: &ApiClient,
    ws: &str,
    game: &str,
    number: i64,
    timeout_secs: u64,
    reporter: &Reporter,
) -> Result<RevisionDetail, CliError> {
    let spinner = reporter.spinner("waiting for bet-stats");
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let detail = client.get_revision(ws, game, number).await?;
        let pending = detail
            .stats
            .as_ref()
            .map(|s| s.status == "pending")
            .unwrap_or(true);
        if !pending {
            spinner.finish("bet-stats ready");
            return Ok(detail);
        }
        if Instant::now() >= deadline {
            spinner.finish("timed out waiting for bet-stats");
            return Ok(detail);
        }
        sleep(STATS_POLL_INTERVAL).await;
    }
}

/// Hashes every manifest entry, showing a spinner while it runs.
fn hash_all(entries: Vec<ManifestEntry>, reporter: &Reporter) -> Result<Vec<HashedFile>, CliError> {
    let spinner = reporter.spinner(&format!("hashing {} file(s)", entries.len()));
    let mut hashed = Vec::with_capacity(entries.len());
    for entry in entries {
        let hash = hash::hash_file(&entry.path)
            .map_err(|e| CliError::usage(anyhow!("failed to read {}: {e}", entry.rel_path)))?;
        hashed.push(HashedFile {
            rel_path: entry.rel_path,
            path: entry.path,
            size: entry.size,
            hash,
        });
    }
    spinner.finish(&format!("hashed {} file(s)", hashed.len()));
    Ok(hashed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiError;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Scripted `create_revision` outcome.
    enum FakeCreate {
        Ok(i64),
        MissingBlobs(Vec<String>),
        StaleParent,
    }

    /// In-memory [`RevisionApi`] that records uploads and replays scripted
    /// commit responses — no network, no disk.
    struct FakeClient {
        missing_on_check: Vec<String>,
        uploaded: Mutex<Vec<String>>,
        create_calls: Mutex<u32>,
        create_responses: Mutex<VecDeque<FakeCreate>>,
        upload_fails: bool,
    }

    impl FakeClient {
        fn new(missing_on_check: Vec<String>, creates: Vec<FakeCreate>) -> Self {
            Self {
                missing_on_check,
                uploaded: Mutex::new(Vec::new()),
                create_calls: Mutex::new(0),
                create_responses: Mutex::new(creates.into()),
                upload_fails: false,
            }
        }

        fn with_upload_error(mut self) -> Self {
            self.upload_fails = true;
            self
        }
    }

    impl RevisionApi for FakeClient {
        async fn check_files(
            &self,
            _ws: &str,
            _game: &str,
            _files: &[FileEntry],
        ) -> ClientResult<Vec<String>> {
            Ok(self.missing_on_check.clone())
        }

        async fn upload_blob(
            &self,
            _ws: &str,
            _game: &str,
            upload: &BlobUpload,
            _progress: FileProgress,
        ) -> ClientResult<()> {
            if self.upload_fails {
                return Err(ClientError::Api(ApiError {
                    status: 400,
                    code: "bad_blob".into(),
                    message: "rejected".into(),
                    missing: vec![],
                }));
            }
            self.uploaded.lock().unwrap().push(upload.hash.clone());
            Ok(())
        }

        async fn create_revision(
            &self,
            _ws: &str,
            _game: &str,
            _req: &CreateRevisionRequest,
        ) -> ClientResult<RevisionDetail> {
            *self.create_calls.lock().unwrap() += 1;
            let next = self
                .create_responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra create_revision call");
            match next {
                FakeCreate::Ok(number) => Ok(detail(number)),
                FakeCreate::MissingBlobs(missing) => Err(ClientError::Api(ApiError {
                    status: 409,
                    code: "missing_blobs".into(),
                    message: "blobs missing".into(),
                    missing,
                })),
                FakeCreate::StaleParent => Err(ClientError::Api(ApiError {
                    status: 409,
                    code: "stale_parent".into(),
                    message: "parent moved on".into(),
                    missing: vec![],
                })),
            }
        }

        async fn get_revision(
            &self,
            _ws: &str,
            _game: &str,
            number: i64,
        ) -> ClientResult<RevisionDetail> {
            Ok(detail(number))
        }
    }

    fn detail(number: i64) -> RevisionDetail {
        RevisionDetail {
            number,
            message: None,
            created_at: None,
            files: vec![],
            stats: None,
            extra: Default::default(),
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
    async fn uploads_only_the_blobs_the_server_is_missing() {
        let hashed = vec![
            hf("index.json", "h1", 10),
            hf("a.csv", "h2", 20),
            hf("b.zst", "h3", 30),
        ];
        let files = files_of(&hashed);
        let fake = FakeClient::new(vec!["h2".into()], vec![FakeCreate::Ok(42)]);
        let reporter = Reporter::new(true);
        let job = PushJob {
            ws: "w",
            game: "g",
            message: "m",
            parent: None,
            hashed: &hashed,
            files: &files,
        };

        let out = execute_push(&fake, &job, &reporter).await.unwrap();

        assert_eq!(out.detail.number, 42);
        assert_eq!(out.uploaded_count, 1);
        assert_eq!(out.uploaded_bytes, 20);
        assert_eq!(out.total_bytes, 60); // dedup saved 40
        assert_eq!(*fake.uploaded.lock().unwrap(), vec!["h2".to_string()]);
    }

    #[tokio::test]
    async fn retries_commit_once_on_missing_blobs() {
        let hashed = vec![hf("index.json", "h1", 10), hf("a.csv", "h2", 20)];
        let files = files_of(&hashed);
        // Nothing missing at check time, but the first commit reports h2 gone.
        let fake = FakeClient::new(
            vec![],
            vec![
                FakeCreate::MissingBlobs(vec!["h2".into()]),
                FakeCreate::Ok(7),
            ],
        );
        let reporter = Reporter::new(true);
        let job = PushJob {
            ws: "w",
            game: "g",
            message: "m",
            parent: None,
            hashed: &hashed,
            files: &files,
        };

        let out = execute_push(&fake, &job, &reporter).await.unwrap();

        assert_eq!(out.detail.number, 7);
        assert_eq!(*fake.create_calls.lock().unwrap(), 2); // retried exactly once
        assert_eq!(*fake.uploaded.lock().unwrap(), vec!["h2".to_string()]);
        assert_eq!(out.uploaded_count, 1);
        assert_eq!(out.uploaded_bytes, 20);
    }

    #[tokio::test]
    async fn aborts_with_exit_1_on_stale_parent() {
        let hashed = vec![hf("index.json", "h1", 10)];
        let files = files_of(&hashed);
        let fake = FakeClient::new(vec![], vec![FakeCreate::StaleParent]);
        let reporter = Reporter::new(true);
        let job = PushJob {
            ws: "w",
            game: "g",
            message: "m",
            parent: Some(3),
            hashed: &hashed,
            files: &files,
        };

        let err = execute_push(&fake, &job, &reporter).await.unwrap_err();
        assert_eq!(err.exit_code(), 1);
    }

    #[tokio::test]
    async fn upload_failure_aborts_with_exit_3() {
        let hashed = vec![hf("index.json", "h1", 10)];
        let files = files_of(&hashed);
        let fake = FakeClient::new(vec!["h1".into()], vec![FakeCreate::Ok(1)]).with_upload_error();
        let reporter = Reporter::new(true);
        let job = PushJob {
            ws: "w",
            game: "g",
            message: "m",
            parent: None,
            hashed: &hashed,
            files: &files,
        };

        let err = execute_push(&fake, &job, &reporter).await.unwrap_err();
        assert_eq!(err.exit_code(), 3); // non-retryable upload error → server class
        assert_eq!(*fake.create_calls.lock().unwrap(), 0); // never reached commit
    }
}
