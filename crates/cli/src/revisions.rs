//! The `sdt revisions` command: a table of a game's revisions.

use anyhow::anyhow;

use crate::RevisionsArgs;
use crate::api::{ApiClient, RevisionList};
use crate::error::CliError;
use crate::output;

pub async fn run(client: &ApiClient, args: RevisionsArgs) -> Result<(), CliError> {
    let value = client
        .list_revisions_raw(&args.workspace, &args.game, args.limit)
        .await?;

    if args.json {
        // CI-facing: echo the server response verbatim to stdout.
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    let mut list: RevisionList = serde_json::from_value(value)
        .map_err(|e| CliError::server(anyhow!("could not parse revisions: {e}")))?;
    // Honour --limit even if the server ignored the query parameter.
    if let Some(limit) = args.limit {
        list.revisions.truncate(limit as usize);
    }

    // The table is human output → stderr.
    eprintln!("{}", output::revisions_table(&list.revisions));
    Ok(())
}
