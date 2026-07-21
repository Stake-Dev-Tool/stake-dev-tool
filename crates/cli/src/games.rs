//! The `sdt games` command: a table of a workspace's games.

use anyhow::anyhow;

use crate::GamesArgs;
use crate::api::{ApiClient, GamesResponse, PlatformApi};
use crate::error::CliError;
use crate::output;

pub async fn run(client: &ApiClient, args: GamesArgs) -> Result<(), CliError> {
    let value = client.list_games(&args.workspace).await?;

    if args.json {
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    let parsed: GamesResponse = serde_json::from_value(value)
        .map_err(|e| CliError::server(anyhow!("could not parse games: {e}")))?;
    eprintln!("{}", output::games_table(&parsed.games));
    Ok(())
}
