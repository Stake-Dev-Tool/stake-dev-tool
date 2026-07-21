//! The top-level CLI error, which pairs a message with the process exit code
//! the spec assigns to that class of failure.

use std::fmt::Display;

use crate::api::ClientError;

/// Documented exit codes. `0` (success) is expressed by returning `Ok`.
#[derive(Debug, Clone, Copy)]
enum ExitKind {
    /// Bad input or local validation (e.g. not a math folder).
    Usage = 1,
    /// Authentication/authorization failure (401/403).
    Auth = 2,
    /// Server or network failure.
    Server = 3,
}

/// An error carrying both a human message and the exit code to terminate with.
#[derive(Debug)]
pub struct CliError {
    kind: ExitKind,
    error: anyhow::Error,
}

impl CliError {
    /// Exit 1 — usage/validation, from an existing error.
    pub fn usage(error: impl Into<anyhow::Error>) -> Self {
        Self {
            kind: ExitKind::Usage,
            error: error.into(),
        }
    }

    /// Exit 1 — usage/validation, from a message.
    pub fn usage_msg(msg: impl Display) -> Self {
        Self {
            kind: ExitKind::Usage,
            error: anyhow::anyhow!("{msg}"),
        }
    }

    /// Exit 2 — authentication/authorization, from a message. The message
    /// should hint at the fix (missing token, missing scope).
    pub fn auth(msg: impl Display) -> Self {
        Self {
            kind: ExitKind::Auth,
            error: anyhow::anyhow!("{msg}"),
        }
    }

    /// Exit 3 — server/network, from an existing error.
    pub fn server(error: impl Into<anyhow::Error>) -> Self {
        Self {
            kind: ExitKind::Server,
            error: error.into(),
        }
    }

    /// The process exit code for this error.
    pub fn exit_code(&self) -> u8 {
        self.kind as u8
    }

    /// Prints the error chain to stderr in the conventional `error: …` form.
    pub fn report(&self) {
        eprintln!("error: {:#}", self.error);
    }
}

/// Maps a transport/API failure to a CLI error and exit code. Auth failures
/// (401/403) become exit 2 with a scope hint; everything else is server (3).
/// Callers that need to intercept a specific `code` (missing_blobs,
/// stale_parent) must do so before falling back to this conversion.
impl From<ClientError> for CliError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::Api(api) => match api.status {
                401 => CliError::auth(format!(
                    "authentication failed: {} — check your token (SDT_TOKEN) or run `sdt login`",
                    api.message
                )),
                403 => CliError::auth(format!(
                    "forbidden: {} — this token needs the push:math scope",
                    api.message
                )),
                _ => CliError::server(anyhow::Error::new(api)),
            },
            other => CliError::server(anyhow::Error::new(other)),
        }
    }
}
