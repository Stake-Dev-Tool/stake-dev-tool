//! `sdt` — the Stake Dev Tool CLI. Its job is to let a math CI pipeline push a
//! math folder to the cloud as a new revision, uploading only changed blobs.
//!
//! Layout: this file is clap definitions + dispatch; [`api`] isolates every
//! wire shape and HTTP call; [`hash`] scans and hashes the folder; [`push`]
//! orchestrates a push; [`output`] renders progress and tables; [`config`]
//! resolves settings; [`login`]/[`revisions`] are the remaining commands.

mod api;
mod config;
mod error;
mod hash;
mod login;
mod output;
mod push;
mod revisions;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::api::ApiClient;
use crate::config::DEFAULT_SERVER;
use crate::error::CliError;

/// Push math revisions to the Stake Dev Tool cloud from CI.
#[derive(Parser)]
#[command(name = "sdt", version, about)]
struct Cli {
    /// Server base URL (env: SDT_SERVER) [default: http://127.0.0.1:8080].
    #[arg(long, global = true, value_name = "URL")]
    server: Option<String>,

    /// API token with the push:math scope (env: SDT_TOKEN).
    #[arg(long, global = true, value_name = "TOKEN")]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate via the device flow; print (and optionally save) a token.
    Login(LoginArgs),
    /// Push a math folder as a new revision, uploading only changed blobs.
    Push(PushArgs),
    /// List a game's revisions.
    Revisions(RevisionsArgs),
}

#[derive(Args)]
pub struct LoginArgs {
    /// Also save the server + token to ~/.config/sdt/config.toml.
    #[arg(long)]
    pub save: bool,
}

#[derive(Args)]
pub struct PushArgs {
    /// Path to the math folder (must contain index.json).
    pub path: PathBuf,

    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// Commit message.
    #[arg(short = 'm', long)]
    pub message: String,

    /// Parent revision number (defaults to the server's latest).
    #[arg(long)]
    pub parent: Option<i64>,

    /// Disable progress bars (auto-disabled when stderr is not a TTY).
    #[arg(long)]
    pub no_progress: bool,

    /// After committing, wait for bet-stats and print them.
    #[arg(long)]
    pub wait_stats: bool,

    /// Seconds to wait for --wait-stats before giving up.
    #[arg(long, default_value_t = 120)]
    pub timeout: u64,

    /// Print the raw revision JSON to stdout (for CI scripting).
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct RevisionsArgs {
    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// Show at most N most-recent revisions.
    #[arg(long)]
    pub limit: Option<u32>,

    /// Print the raw JSON to stdout (for CI scripting).
    #[arg(long)]
    pub json: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    // clap handles --help/--version and argument errors itself (exiting 0/2).
    let cli = Cli::parse();
    match dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            err.report();
            ExitCode::from(err.exit_code())
        }
    }
}

async fn dispatch(cli: Cli) -> Result<(), CliError> {
    // Precedence: flag > env > config file, with a built-in server default.
    let file = config::load_file();
    let server = config::pick(cli.server, env_opt("SDT_SERVER"), file.server)
        .unwrap_or_else(|| DEFAULT_SERVER.to_string());
    let token = config::pick(cli.token, env_opt("SDT_TOKEN"), file.token);

    match cli.command {
        Command::Login(args) => {
            let client = ApiClient::new(&server, token).map_err(CliError::server)?;
            login::run(&client, args, &server).await
        }
        Command::Push(args) => {
            let client =
                ApiClient::new(&server, Some(require_token(token)?)).map_err(CliError::server)?;
            push::run(&client, args).await
        }
        Command::Revisions(args) => {
            let client =
                ApiClient::new(&server, Some(require_token(token)?)).map_err(CliError::server)?;
            revisions::run(&client, args).await
        }
    }
}

/// The authenticated commands need a token up front, with a helpful message.
fn require_token(token: Option<String>) -> Result<String, CliError> {
    token.ok_or_else(|| {
        CliError::auth("no API token found; set SDT_TOKEN, pass --token, or run `sdt login --save`")
    })
}

/// Reads an environment variable, treating an empty value as unset.
fn env_opt(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}
