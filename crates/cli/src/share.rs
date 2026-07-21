//! The `sdt share` commands: create a share link, list a game's links, and
//! revoke one. A share link is a real hosted game instance on its own
//! `<slug>.play.<domain>` subdomain; creating one and sending the URL is the
//! whole point of the workflow.
//!
//! Share CRUD is gated on the workspace **owner/admin** role (see
//! `crates/server/src/api/shares.rs`), which is a different gate from the
//! front-bundle push: a `push:math`-scoped token is enough to push a bundle but
//! NOT to manage links, so [`map_share_err`] gives 403s a role-specific hint.

use anyhow::anyhow;

use crate::api::{
    ApiClient, ClientError, CreateShareRequest, ShareApi, ShareLinksResponse, UpdateShareRequest,
};
use crate::error::CliError;
use crate::output;
use crate::{ShareCreateArgs, ShareListArgs, ShareRevokeArgs};

/// `sdt share create` — create a share link and surface its URL prominently.
pub async fn create(client: &ApiClient, args: ShareCreateArgs) -> Result<(), CliError> {
    let req = CreateShareRequest {
        slug: args.slug,
        revision_number: args.rev,
        front_bundle_id: args.bundle,
        password: args.password,
        expires_in_days: args.expires_days,
        max_concurrent_sessions: args.max_sessions,
    };

    let view = client
        .create_share(&args.workspace, &args.game, &req)
        .await
        .map_err(map_share_err)?;

    if args.json {
        // CI-facing: the full link view on stdout, nothing else.
        let json = serde_json::to_string_pretty(&view)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    match &view.url {
        Some(url) => {
            // The URL is the machine-usable result → stdout.
            println!("{url}");
            eprintln!("Share link ready — send this URL:\n  {url}");
        }
        None => {
            // No play domain configured: there is no public URL. Fall back to the
            // slug as the machine handle and say so explicitly.
            println!("{}", view.slug);
            eprintln!(
                "Share link created (slug: {}).\n\
                 This instance has no play domain configured (SERVER_PLAY_DOMAIN), \
                 so there is no public URL yet.",
                view.slug
            );
        }
    }
    eprintln!("{}", output::share_settings(&view));
    Ok(())
}

/// `sdt share list` — a table of a game's share links with counters.
pub async fn list(client: &ApiClient, args: ShareListArgs) -> Result<(), CliError> {
    let value = client
        .list_shares(&args.workspace, &args.game)
        .await
        .map_err(map_share_err)?;

    if args.json {
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    let parsed: ShareLinksResponse = serde_json::from_value(value)
        .map_err(|e| CliError::server(anyhow!("could not parse shares: {e}")))?;
    eprintln!("{}", output::shares_table(&parsed.shares));
    Ok(())
}

/// `sdt share revoke <id-or-slug>` — resolve the link (by id first, else slug)
/// from the list, then PATCH it revoked.
pub async fn revoke(client: &ApiClient, args: ShareRevokeArgs) -> Result<(), CliError> {
    let value = client
        .list_shares(&args.workspace, &args.game)
        .await
        .map_err(map_share_err)?;
    let parsed: ShareLinksResponse = serde_json::from_value(value)
        .map_err(|e| CliError::server(anyhow!("could not parse shares: {e}")))?;

    let mut shares = parsed.shares;
    // Resolve by id first, then by slug.
    let pos = shares
        .iter()
        .position(|s| s.id == args.target)
        .or_else(|| shares.iter().position(|s| s.slug == args.target));
    let Some(pos) = pos else {
        return Err(CliError::usage_msg(format!(
            "no share link with id or slug '{}' in {}/{}",
            args.target, args.workspace, args.game
        )));
    };
    let share = shares.swap_remove(pos);

    if share.revoked_at.is_some() {
        eprintln!("Share {} ({}) is already revoked.", share.slug, share.id);
        println!("{}", share.id);
        return Ok(());
    }

    let req = UpdateShareRequest {
        revoked: Some(true),
    };
    let view = client
        .update_share(&args.workspace, &args.game, &share.id, &req)
        .await
        .map_err(map_share_err)?;

    eprintln!("Revoked share {} ({}).", view.slug, view.id);
    // The id is the machine-usable result → stdout.
    println!("{}", view.id);
    Ok(())
}

/// Maps a share-CRUD failure to a CLI error. A 403 here means the caller is a
/// workspace member but not an owner/admin — distinct from the front-bundle
/// push, whose 403 means a missing `push:math` scope — so the hint points at the
/// role, and notes that a `push:math` token is not sufficient.
fn map_share_err(e: ClientError) -> CliError {
    match e {
        ClientError::Api(api) if api.status == 403 => CliError::auth(format!(
            "forbidden: {} — managing share links requires the workspace owner or admin role \
             (unlike push-front, a push:math-scoped token is not enough)",
            api.message
        )),
        other => other.into(),
    }
}
