use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::auth::load_token;

const API_BASE: &str = "https://api.github.com";

#[derive(Clone)]
pub struct GithubClient {
    http: reqwest::Client,
    token: String,
}

impl GithubClient {
    pub fn from_stored_token() -> Result<Self> {
        let token = load_token()?.ok_or_else(|| anyhow!("not signed in to GitHub"))?;
        let http = reqwest::Client::builder()
            .user_agent("stake-dev-tool")
            .timeout(Duration::from_secs(60))
            .build()
            .context("build http client")?;
        Ok(Self { http, token })
    }

    fn json_request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    pub async fn get_repo(&self, owner: &str, name: &str) -> Result<RepoInfo> {
        let url = format!("{API_BASE}/repos/{owner}/{name}");
        let res = self
            .json_request(reqwest::Method::GET, &url)
            .send()
            .await
            .context("get repo")?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("get repo {owner}/{name}: {status} {body}"));
        }
        res.json().await.context("parse repo")
    }

    // ============================================================
    // Git Data API — used to push many files as a single commit.
    // The Contents API forces one commit per file, which is dog slow for
    // bundle uploads (each commit is a network round-trip). Switching to
    // Git Data lets us parallelise blob uploads and stitch them into a
    // single tree + commit, matching `git push` semantics.
    // ============================================================

    pub async fn get_branch_head(
        &self,
        owner: &str,
        name: &str,
        branch: &str,
    ) -> Result<BranchHead> {
        let url = format!("{API_BASE}/repos/{owner}/{name}/branches/{branch}");
        let res = self
            .json_request(reqwest::Method::GET, &url)
            .send()
            .await
            .context("get branch head")?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("get branch {branch}: {status} {body}"));
        }
        #[derive(Deserialize)]
        struct CommitInner {
            sha: String,
            commit: CommitBody,
        }
        #[derive(Deserialize)]
        struct CommitBody {
            tree: TreeRef,
        }
        #[derive(Deserialize)]
        struct TreeRef {
            sha: String,
        }
        #[derive(Deserialize)]
        struct Resp {
            commit: CommitInner,
        }
        let r: Resp = res.json().await.context("parse branch")?;
        Ok(BranchHead {
            commit_sha: r.commit.sha,
            tree_sha: r.commit.commit.tree.sha,
        })
    }

    /// Upload `bytes` as a blob, returns its SHA. Always base64-encoded so
    /// binary files (.wasm, fonts, images, …) round-trip cleanly.
    pub async fn create_blob(&self, owner: &str, name: &str, bytes: &[u8]) -> Result<String> {
        let url = format!("{API_BASE}/repos/{owner}/{name}/git/blobs");
        let res = self
            .json_request(reqwest::Method::POST, &url)
            .json(&serde_json::json!({
                "content": B64.encode(bytes),
                "encoding": "base64",
            }))
            .send()
            .await
            .context("create blob")?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("create blob: {status} {body}"));
        }
        #[derive(Deserialize)]
        struct Resp {
            sha: String,
        }
        let r: Resp = res.json().await.context("parse blob")?;
        Ok(r.sha)
    }

    /// Build a new tree on top of `base_tree`, overlaying `entries`. Each
    /// entry's `path` is relative to the repo root.
    pub async fn create_tree(
        &self,
        owner: &str,
        name: &str,
        base_tree: &str,
        entries: &[GitTreeEntry],
    ) -> Result<String> {
        let url = format!("{API_BASE}/repos/{owner}/{name}/git/trees");
        let res = self
            .json_request(reqwest::Method::POST, &url)
            .json(&serde_json::json!({
                "base_tree": base_tree,
                "tree": entries,
            }))
            .send()
            .await
            .context("create tree")?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("create tree: {status} {body}"));
        }
        #[derive(Deserialize)]
        struct Resp {
            sha: String,
        }
        let r: Resp = res.json().await.context("parse tree")?;
        Ok(r.sha)
    }

    pub async fn create_commit(
        &self,
        owner: &str,
        name: &str,
        message: &str,
        tree_sha: &str,
        parents: &[&str],
    ) -> Result<String> {
        let url = format!("{API_BASE}/repos/{owner}/{name}/git/commits");
        let res = self
            .json_request(reqwest::Method::POST, &url)
            .json(&serde_json::json!({
                "message": message,
                "tree": tree_sha,
                "parents": parents,
            }))
            .send()
            .await
            .context("create commit")?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("create commit: {status} {body}"));
        }
        #[derive(Deserialize)]
        struct Resp {
            sha: String,
        }
        let r: Resp = res.json().await.context("parse commit")?;
        Ok(r.sha)
    }

    pub async fn update_ref(
        &self,
        owner: &str,
        name: &str,
        branch: &str,
        commit_sha: &str,
    ) -> Result<()> {
        let url = format!("{API_BASE}/repos/{owner}/{name}/git/refs/heads/{branch}");
        let res = self
            .json_request(reqwest::Method::PATCH, &url)
            .json(&serde_json::json!({ "sha": commit_sha, "force": false }))
            .send()
            .await
            .context("update ref")?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("update ref {branch}: {status} {body}"));
        }
        Ok(())
    }

    /// Permanently delete a repo. Caller must have admin rights. Irreversible.
    pub async fn delete_repo(&self, owner: &str, name: &str) -> Result<()> {
        let url = format!("{API_BASE}/repos/{owner}/{name}");
        let res = self
            .json_request(reqwest::Method::DELETE, &url)
            .send()
            .await
            .context("delete repo")?;
        let status = res.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(());
        }
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("delete repo {owner}/{name}: {status} {body}"));
        }
        Ok(())
    }

    /// Fetch a file's content + SHA. Returns None if the file doesn't exist.
    pub async fn get_file(&self, owner: &str, name: &str, path: &str) -> Result<Option<RepoFile>> {
        let url = format!("{API_BASE}/repos/{owner}/{name}/contents/{path}");
        let res = self
            .json_request(reqwest::Method::GET, &url)
            .send()
            .await
            .context("get file")?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("get file {path}: {status} {body}"));
        }
        let raw: RepoFileRaw = res.json().await.context("parse file")?;
        let content = B64
            .decode(raw.content.replace('\n', ""))
            .context("decode file content")?;
        Ok(Some(RepoFile {
            sha: raw.sha,
            content,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    pub private: bool,
    pub html_url: String,
    pub owner: RepoOwner,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoOwner {
    pub login: String,
    pub id: u64,
}

#[derive(Debug, Clone)]
pub struct BranchHead {
    pub commit_sha: String,
    pub tree_sha: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitTreeEntry {
    pub path: String,
    pub mode: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub sha: String,
}

impl GitTreeEntry {
    pub fn blob(path: String, sha: String) -> Self {
        Self {
            path,
            mode: "100644".to_string(),
            kind: "blob".to_string(),
            sha,
        }
    }
}

/// A fetched repo file. Currently `get_file` is used only as a readiness probe
/// (preview's `create_public_repo` waits for the contents endpoint), so the
/// fields are retained for the API shape but not read — hence `dead_code`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RepoFile {
    pub sha: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, Deserialize)]
struct RepoFileRaw {
    sha: String,
    content: String,
}
