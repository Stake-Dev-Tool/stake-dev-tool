//! Shared wire types for the Stake Dev Tool cloud platform.
//!
//! Every public type derives `serde` (de)serialization and `ts_rs::TS` so the
//! server and the SvelteKit UI speak one source of truth. Running
//! `cargo test -p protocol` regenerates the TypeScript bindings under
//! `ui/src/lib/protocol/`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod admin;
pub mod auth;
pub mod billing;
pub mod documents;
pub mod error;
pub mod math;
pub mod shares;
pub mod workspace;

pub use admin::*;
pub use auth::*;
pub use documents::*;
pub use error::*;
pub use math::*;
pub use workspace::*;

/// Health of a single dependency the server talks to (database, object store).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ComponentStatus {
    pub ok: bool,
    pub error: Option<String>,
}

/// Overall service health. `degraded` means at least one component is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "protocol/")]
pub enum ServiceStatus {
    Ok,
    Degraded,
}

/// Response body of `GET /healthz`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct HealthResponse {
    pub status: ServiceStatus,
    pub version: String,
    /// Git commit the running binary was built from (`SERVER_BUILD_SHA`,
    /// stamped by the deploy image). Absent on ad-hoc local builds.
    #[serde(default)]
    pub build: Option<String>,
    pub db: ComponentStatus,
    pub object_store: ComponentStatus,
}
