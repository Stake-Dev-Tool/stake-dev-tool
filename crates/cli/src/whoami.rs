//! The `sdt whoami` command: show the authenticated user (GET /api/auth/me).

use anyhow::anyhow;

use crate::WhoamiArgs;
use crate::api::ApiClient;
use crate::error::CliError;

pub async fn run(client: &ApiClient, args: WhoamiArgs) -> Result<(), CliError> {
    let value = client.get_me().await?;

    if args.json {
        // Machine-readable: echo the server response verbatim to stdout.
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| CliError::server(anyhow!("could not encode response: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    // Human output → stderr. The envelope is `{ "user": { display_name, email } }`.
    let user = value.get("user").unwrap_or(&value);
    let name = user
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("(unknown)");
    let email = user.get("email").and_then(|v| v.as_str()).unwrap_or("-");
    eprintln!("Signed in as {name} <{email}>");
    Ok(())
}
