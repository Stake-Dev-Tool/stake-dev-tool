//! `sdt` — the Stake Dev Tool CLI. Its job is to let a math CI pipeline push a
//! math folder to the cloud as a new revision, uploading only changed blobs.
//!
//! Layout: this file is clap definitions + dispatch; [`api`] isolates every
//! wire shape and HTTP call; [`hash`] scans and hashes the folder; [`push`]
//! orchestrates a push; [`output`] renders progress and tables; [`config`]
//! resolves settings; [`login`]/[`revisions`] are the remaining commands.

mod api;
mod config;
mod diff;
mod error;
mod front;
mod games;
mod hash;
mod login;
mod mcp;
mod output;
mod pull;
mod push;
mod revisions;
mod share;
mod stats;
mod whoami;
mod workspaces;

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
    /// Show the authenticated user (display name + email).
    Whoami(WhoamiArgs),
    /// List the workspaces the token can access.
    Workspaces(WorkspacesArgs),
    /// List a workspace's games.
    Games(GamesArgs),
    /// Push a math folder as a new revision, uploading only changed blobs.
    Push(PushArgs),
    /// Push a front-bundle folder (a web build) as a new bundle.
    PushFront(PushFrontArgs),
    /// Manage share links: create, list, and revoke.
    Share(ShareArgs),
    /// List a game's revisions.
    Revisions(RevisionsArgs),
    /// Show a revision's per-mode bet-stats (defaults to the head revision).
    Stats(StatsArgs),
    /// Diff two revisions: file summary and per-mode RTP deltas.
    Diff(DiffArgs),
    /// Download a revision's files to a directory.
    Pull(PullArgs),
    /// Run a Model Context Protocol server over stdio (for MCP clients).
    #[command(
        long_about = "Run a Model Context Protocol (MCP) server on stdin/stdout so an MCP \
client (Claude Code, IDEs) can drive the platform.\n\n\
The server URL and token come from the usual precedence: --server/--token flags, then \
SDT_SERVER/SDT_TOKEN, then ~/.config/sdt/config.toml. A token is required to start; the \
push_math tool additionally needs the push:math scope.\n\n\
Register in Claude Code:\n  \
claude mcp add sdt -e SDT_TOKEN=<token> -- sdt mcp --server https://app.stakedevtool.com"
    )]
    Mcp,
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
pub struct PushFrontArgs {
    /// Path to the front-bundle folder (must contain index.html at its root).
    pub path: PathBuf,

    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// Disable progress bars (auto-disabled when stderr is not a TTY).
    #[arg(long)]
    pub no_progress: bool,

    /// Print a machine-readable recap JSON to stdout (for CI scripting).
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ShareArgs {
    #[command(subcommand)]
    pub command: ShareCommand,
}

#[derive(Subcommand)]
pub enum ShareCommand {
    /// Create a share link and print its URL.
    Create(ShareCreateArgs),
    /// List a game's share links with counters and observed RTP.
    List(ShareListArgs),
    /// Revoke a share link by id or slug.
    Revoke(ShareRevokeArgs),
}

#[derive(Args)]
pub struct ShareCreateArgs {
    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// Custom subdomain label (generated `word-word-nnn` when omitted).
    #[arg(long)]
    pub slug: Option<String>,

    /// Pin a revision number (omit to track the game's latest revision).
    #[arg(long)]
    pub rev: Option<i64>,

    /// Pin a front bundle by id (omit to serve the game's latest bundle).
    #[arg(long)]
    pub bundle: Option<String>,

    /// Password-protect the link (visitors must enter it).
    #[arg(long)]
    pub password: Option<String>,

    /// Expire the link this many days from now (omit for no expiry).
    #[arg(long = "expires-days")]
    pub expires_days: Option<i64>,

    /// Concurrent visitor-session cap (omit for the server default of 25).
    #[arg(long = "max-sessions")]
    pub max_sessions: Option<i64>,

    /// Print the raw share JSON to stdout (for CI scripting).
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ShareListArgs {
    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// Print the raw JSON to stdout (for CI scripting).
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ShareRevokeArgs {
    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// The share link to revoke, by id or slug.
    #[arg(value_name = "SHARE")]
    pub target: String,
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

#[derive(Args)]
pub struct WhoamiArgs {
    /// Print the raw JSON to stdout.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct WorkspacesArgs {
    /// Print the raw JSON to stdout.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct GamesArgs {
    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Print the raw JSON to stdout.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct StatsArgs {
    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// Revision number (defaults to the head revision).
    #[arg(long)]
    pub rev: Option<i64>,

    /// Poll while bet-stats are pending instead of returning immediately.
    #[arg(long)]
    pub wait: bool,

    /// Seconds to wait for --wait before giving up.
    #[arg(long, default_value_t = 120)]
    pub timeout: u64,

    /// Print the raw revision JSON to stdout.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct DiffArgs {
    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// The "after" revision number (A in `diff A B`).
    #[arg(value_name = "A")]
    pub after: i64,

    /// The "before" revision number (B in `diff A B`).
    #[arg(value_name = "B")]
    pub before: i64,

    /// Print the raw JSON to stdout.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct PullArgs {
    /// Workspace slug.
    #[arg(long)]
    pub workspace: String,

    /// Game slug.
    #[arg(long)]
    pub game: String,

    /// Revision number (defaults to the head revision).
    #[arg(long)]
    pub rev: Option<i64>,

    /// Output directory (default: ./<game>-rev<N>).
    #[arg(short = 'o', long = "output", value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// Overwrite even when the destination directory is not empty.
    #[arg(long)]
    pub force: bool,
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
        Command::Whoami(args) => {
            let client = authed_client(&server, token)?;
            whoami::run(&client, args).await
        }
        Command::Workspaces(args) => {
            let client = authed_client(&server, token)?;
            workspaces::run(&client, args).await
        }
        Command::Games(args) => {
            let client = authed_client(&server, token)?;
            games::run(&client, args).await
        }
        Command::Push(args) => {
            let client = authed_client(&server, token)?;
            push::run(&client, args).await
        }
        Command::PushFront(args) => {
            let client = authed_client(&server, token)?;
            front::run(&client, args).await
        }
        Command::Share(args) => {
            let client = authed_client(&server, token)?;
            match args.command {
                ShareCommand::Create(args) => share::create(&client, args).await,
                ShareCommand::List(args) => share::list(&client, args).await,
                ShareCommand::Revoke(args) => share::revoke(&client, args).await,
            }
        }
        Command::Revisions(args) => {
            let client = authed_client(&server, token)?;
            revisions::run(&client, args).await
        }
        Command::Stats(args) => {
            let client = authed_client(&server, token)?;
            stats::run(&client, args).await
        }
        Command::Diff(args) => {
            let client = authed_client(&server, token)?;
            diff::run(&client, args).await
        }
        Command::Pull(args) => {
            let client = authed_client(&server, token)?;
            pull::run(&client, args).await
        }
        Command::Mcp => {
            let client = authed_client(&server, token)?;
            mcp::run(client).await
        }
    }
}

/// Builds an [`ApiClient`] that requires a token up front, with a helpful error.
fn authed_client(server: &str, token: Option<String>) -> Result<ApiClient, CliError> {
    ApiClient::new(server, Some(require_token(token)?)).map_err(CliError::server)
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
