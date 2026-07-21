//! The `sdt diff` command: file + per-mode stats diff between two revisions.
//!
//! `sdt diff … <A> <B>` maps to `GET …/revisions/A/diff/B`, where the server
//! treats `A` as "after" (`:number`) and `B` as "before" (`:other`).

use anyhow::anyhow;

use crate::DiffArgs;
use crate::api::{ApiClient, PlatformApi, RevisionDiff};
use crate::error::CliError;
use crate::output;

pub async fn run(client: &ApiClient, args: DiffArgs) -> Result<(), CliError> {
    // `after` = A (:number), `before` = B (:other).
    let value = client
        .get_diff(&args.workspace, &args.game, args.after, args.before)
        .await?;

    if args.json {
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    let diff: RevisionDiff = serde_json::from_value(value)
        .map_err(|e| CliError::server(anyhow!("could not parse diff: {e}")))?;

    let color = output::colors_enabled();
    eprintln!(
        "Diff: revision #{} (after) vs #{} (before)",
        args.after, args.before
    );
    eprintln!("{}", output::diff_files_summary(&diff.files, color));
    eprintln!();
    eprintln!(
        "{}",
        output::diff_stats_table(&diff.stats, args.before, args.after, color)
    );
    Ok(())
}
