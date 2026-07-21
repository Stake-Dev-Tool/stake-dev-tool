//! The `sdt stats` command: a revision's per-mode bet-stats table.

use anyhow::anyhow;

use crate::StatsArgs;
use crate::api::{ApiClient, RevisionApi, RevisionDetail, resolve_head};
use crate::error::CliError;
use crate::output::{self, Reporter};
use crate::push;

pub async fn run(client: &ApiClient, args: StatsArgs) -> Result<(), CliError> {
    // Default to the head revision, resolved via the revisions list.
    let number = match args.rev {
        Some(n) => n,
        None => resolve_head(client, &args.workspace, &args.game).await?,
    };

    let mut detail = client
        .get_revision(&args.workspace, &args.game, number)
        .await?;

    if args.wait && is_pending(&detail) {
        let reporter = Reporter::new(false);
        detail = push::wait_for_stats(
            client,
            &args.workspace,
            &args.game,
            number,
            args.timeout,
            &reporter,
        )
        .await?;
    }

    if args.json {
        let json = serde_json::to_string_pretty(&detail)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    // Human output → stderr.
    let message = detail.message.as_deref().unwrap_or("");
    let age = output::format_age(detail.created_at.as_deref());
    eprintln!("Revision #{number} — {message} ({age} ago)");
    eprintln!("{}", output::stats_table(&detail));
    if is_pending(&detail) {
        eprintln!("Bet-stats are still pending; re-run with --wait or try again later.");
    }
    Ok(())
}

fn is_pending(detail: &RevisionDetail) -> bool {
    detail
        .stats
        .as_ref()
        .map(|s| s.status == "pending")
        .unwrap_or(true)
}
