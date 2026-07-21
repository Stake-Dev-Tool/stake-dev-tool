//! The `sdt login` command: the OAuth-device-flow pairing that mints an API
//! token for CI.

use std::time::Duration;

use tokio::time::{Instant, sleep};

use crate::LoginArgs;
use crate::api::{ApiClient, ClientError};
use crate::config;
use crate::error::CliError;

/// Fallbacks when the server omits the RFC 8628 timing fields.
const DEFAULT_INTERVAL: u64 = 5;
const DEFAULT_EXPIRES_IN: u64 = 900;
/// How much to back off when the server says `slow_down`.
const SLOW_DOWN_STEP: u64 = 5;

pub async fn run(client: &ApiClient, args: LoginArgs, server: &str) -> Result<(), CliError> {
    let code = client.device_code().await?;

    // Human instructions to stderr; kept prominent so they're hard to miss.
    eprintln!();
    eprintln!("To authenticate, visit:");
    eprintln!();
    eprintln!("    {}", code.verification_uri);
    eprintln!();
    eprintln!("and enter the code:  {}", code.user_code);
    eprintln!();
    eprintln!("Waiting for approval… (Ctrl-C to cancel)");

    let mut interval = if code.interval > 0 {
        code.interval as u64
    } else {
        DEFAULT_INTERVAL
    };
    let expires = if code.expires_in > 0 {
        code.expires_in as u64
    } else {
        DEFAULT_EXPIRES_IN
    };
    let deadline = Instant::now() + Duration::from_secs(expires);

    let token = loop {
        sleep(Duration::from_secs(interval)).await;
        match client.device_token(&code.device_code).await {
            Ok(success) => break success.token,
            Err(ClientError::Api(api)) => match api.code.as_str() {
                // Not approved yet — keep polling.
                "authorization_pending" => {}
                // Server asks us to poll less often.
                "slow_down" => interval += SLOW_DOWN_STEP,
                "expired_token" => {
                    return Err(CliError::auth(
                        "the device code expired before it was approved; run `sdt login` again",
                    ));
                }
                "access_denied" => {
                    return Err(CliError::auth("authorization was denied"));
                }
                _ => return Err(ClientError::Api(api).into()),
            },
            // A transient network blip shouldn't kill a login mid-approval;
            // keep trying until the deadline below trips.
            Err(ClientError::Transport(_)) => {}
            Err(other) => return Err(other.into()),
        }
        if Instant::now() >= deadline {
            return Err(CliError::auth(
                "timed out waiting for approval; run `sdt login` again",
            ));
        }
    };

    // The token is the machine-usable result → stdout, printed exactly once.
    println!("{token}");

    eprintln!();
    eprintln!("Login successful.");
    eprintln!("Use it in CI by exporting:");
    eprintln!();
    eprintln!("    export SDT_TOKEN={token}");
    eprintln!();
    if args.save {
        let path = config::save(server, &token).map_err(CliError::server)?;
        eprintln!("Saved server + token to {}", path.display());
    } else if let Some(path) = config::config_path() {
        eprintln!("Re-run with --save to store it in {}", path.display());
    }

    Ok(())
}
