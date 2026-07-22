pub mod api;
pub mod auth;

pub use auth::{DeviceCode, GithubUser};

/// OAuth App client ID. Device Flow doesn't use the client secret, so this
/// string is public and safe to ship in source — GitHub CLI, VS Code, and
/// countless other OSS desktop apps hardcode theirs the same way.
pub const OAUTH_CLIENT_ID: &str = "Ov23liEQ8WQsoUmRg6wg";

/// Scopes needed by the GitHub-Pages share preview (the only remaining GitHub
/// feature after V2 removed the legacy teams system):
///   - `repo` — create the per-preview repo and push the bundle (Git Data API)
///   - `read:user` — fetch the authenticated user's login for the Pages URL
///   - `delete_repo` — delete a preview repo on unpublish
pub const OAUTH_SCOPES: &str = "repo read:user delete_repo";
