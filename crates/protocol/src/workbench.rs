//! Wire types for the workbench capability tokens (the cross-origin LGS mount).
//!
//! The cloud workbench serves the test view and the game front from one origin,
//! so the session cookie authorizes the tenant mount under `/api/ws/…`. When the
//! front instead runs on a developer's own dev server, its calls are cross-site
//! and that cookie is never attached. The test view then mints one of these
//! tokens and hands the game a `/api/wb/<token>` prefix, which carries the
//! authorization in the path instead.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// `POST /api/workbench-tokens` request: the (workspace, game, revision) the
/// token should authorize. The caller must be a member of the workspace.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct WorkbenchTokenRequest {
    /// Workspace slug.
    pub workspace: String,
    /// Game slug within that workspace.
    pub game: String,
    /// Revision number to pin the mount to.
    pub revision: i32,
}

/// A freshly minted workbench token. The secret is returned exactly once.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct WorkbenchToken {
    /// The bearer secret (`sdt_wb_…`). Already embedded in `prefix`.
    pub token: String,
    /// Mount prefix the game front must call instead of
    /// `/api/ws/<ws>/g/<game>/r/<n>` — e.g. `/api/wb/sdt_wb_…`.
    pub prefix: String,
    pub expires_at: DateTime<Utc>,
}
