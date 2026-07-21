//! Cloud plumbing for the V2 platform — the M3-preparation seam described in
//! `docs/v2/recon-m3-m6.md` A.5.
//!
//! This module is **purely additive**: it does not touch the existing
//! GitHub-repo teams system (`crate::teams`, `crate::github`, `crate::math_sync`),
//! which keeps working untouched until M3 proper replaces it. The three
//! submodules mirror the house GitHub patterns one-for-one:
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
