//! Cloud plumbing for the V2 platform — the workspaces team system that
//! replaced the legacy GitHub-repo teams outright in V2.
//!
//! The submodules mirror the house GitHub patterns one-for-one:
//!
//! - [`config`] — the cloud base URL (env > config file > default), so
//!   self-hosters can point the desktop app at their own instance.
//! - [`auth`] — device-flow sign-in against the server, storing the minted API
//!   token in the OS keyring (a cloud-specific keyring user).
//! - [`api`] — [`api::CloudClient`], a bearer-authenticated HTTP client
//!   mirroring [`crate::github::api::GithubClient`], covering the M1 server
//!   surface (workspaces + invites).

pub mod api;
pub mod auth;
pub mod config;
pub mod documents;
pub mod http;
pub mod math;
pub mod sidecar;
pub mod sse;
pub mod sync;
pub mod workspaces;
