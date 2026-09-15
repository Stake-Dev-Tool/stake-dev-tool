//! Append saved rounds to the revision store used by the cloud workbench.
use serde::Deserialize;
use serde_json::{Value, json};

use crate::PushRoundsArgs;
use crate::api::{ApiClient, SavedRoundInput, resolve_head};
use crate::error::CliError;

#[derive(Deserialize)]
#[serde(untagged)]
enum RoundFile {
    Array(Vec<SourceRound>),
    Object { rounds: Vec<SourceRound> },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceRound {
    #[serde(default)]
    game_slug: Option<String>,
    mode: String,
    event_id: u32,
    #[serde(default)]
    description: String,
}

pub async fn run(client: &ApiClient, args: PushRoundsArgs) -> Result<(), CliError> {
    for (flag, slug) in [("--workspace", &args.workspace), ("--game", &args.game)] {
        if slug.is_empty()
            || !slug
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(CliError::usage_msg(format!(
                "{flag} must contain only ASCII letters, digits, hyphens or underscores"
            )));
        }
    }
    if args.rev.is_some_and(|rev| rev <= 0) {
        return Err(CliError::usage_msg(
            "--rev must be a positive revision number",
        ));
    }
    let bytes = tokio::fs::read(&args.path).await.map_err(CliError::usage)?;
    let file: RoundFile = serde_json::from_slice(&bytes).map_err(CliError::usage)?;
    let source = match file {
        RoundFile::Array(rounds) | RoundFile::Object { rounds } => rounds,
    };
    let rounds: Vec<SavedRoundInput> = source
        .into_iter()
        .map(|round| SavedRoundInput {
            game_slug: round.game_slug.unwrap_or_else(|| args.game.clone()),
            mode: round.mode,
            event_id: round.event_id,
            description: round.description,
        })
        .collect();
    for (index, round) in rounds.iter().enumerate() {
        if round.game_slug != args.game {
            return Err(CliError::usage_msg(format!(
                "round {}: gameSlug must match --game '{}' (split multi-game exports first)",
                index + 1,
                args.game
            )));
        }
        if round.mode.trim().is_empty() || round.event_id == 0 {
            return Err(CliError::usage_msg(format!(
                "round {}: mode must be non-blank and eventId must be > 0",
                index + 1
            )));
        }
    }
    let revision = match args.rev {
        Some(revision) => revision,
        None => i32::try_from(resolve_head(client, &args.workspace, &args.game).await?)
            .map_err(CliError::server)?,
    };
    let listed = client
        .saved_rounds(&args.workspace, &args.game, revision, None)
        .await?;
    let mut known: Vec<SavedRoundInput> =
        serde_json::from_value(listed["rounds"].clone()).map_err(CliError::server)?;
    let mut created = Vec::new();
    let mut skipped = 0;
    for round in &rounds {
        if known.contains(round) {
            skipped += 1;
            continue;
        }
        match client
            .saved_rounds(&args.workspace, &args.game, revision, Some(round))
            .await
        {
            Ok(saved) => created.push(saved),
            Err(error) => {
                eprintln!(
                    "Push interrupted: {} confirmed creates, {skipped} skipped. The failed request may have been saved. Inspect the destination, then rerun with --rev {revision}; exact matches are skipped. No automatic POST retry was attempted.",
                    created.len()
                );
                return Err(error.into());
            }
        }
        known.push(round.clone());
    }
    let listed = client
        .saved_rounds(&args.workspace, &args.game, revision, None)
        .await
        .map_err(|error| {
            eprintln!("Read-back failed after {} confirmed creates, {skipped} skipped; inspect the destination before rerunning with --rev {revision}.", created.len());
            CliError::from(error)
        })?;
    let remote: Vec<Value> = serde_json::from_value(listed["rounds"].clone()).map_err(|error| {
        CliError::server(anyhow::anyhow!("read-back response invalid after {} confirmed creates; inspect the destination before rerunning with --rev {revision}: {error}", created.len()))
    })?;
    if created.iter().any(|round| !remote.contains(round)) {
        return Err(CliError::server(anyhow::anyhow!(
            "saved-round read-back verification failed after {} confirmed creates; inspect the destination before rerunning with --rev {revision}",
            created.len()
        )));
    }
    if args.json {
        println!(
            "{}",
            json!({"workspace":args.workspace,"game":args.game,"revision":revision,"created":created.len(),"skipped":skipped})
        );
    } else {
        eprintln!(
            "Saved {} rounds to {}/{} revision #{}",
            created.len(),
            args.workspace,
            args.game,
            revision
        );
    }
    Ok(())
}
