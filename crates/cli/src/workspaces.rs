//! The `sdt workspaces` command: a table of the caller's workspaces.

use anyhow::anyhow;

use crate::WorkspacesArgs;
use crate::api::{ApiClient, PlatformApi, WorkspacesResponse};
use crate::error::CliError;
use crate::output;

pub async fn run(client: &ApiClient, args: WorkspacesArgs) -> Result<(), CliError> {
    let value = client.list_workspaces().await?;

    if args.json {
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    let parsed: WorkspacesResponse = serde_json::from_value(value)
        .map_err(|e| CliError::server(anyhow!("could not parse workspaces: {e}")))?;
    eprintln!("{}", output::workspaces_table(&parsed.workspaces));
    Ok(())
}
