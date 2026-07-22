//! Transactional email via the Resend HTTP API (config-gated; no new deps —
//! `reqwest` already ships with the `json` feature).
//!
//! Every send is best-effort from the caller's point of view: a failure is
//! logged and returned as `ApiError::internal`, and the flows that use it (email
//! verification, password reset) deliberately swallow that error so a mail
//! outage never reveals account state or blocks the user.

use crate::config::MailConfig;
use crate::error::{ApiError, ApiResult};

const RESEND_ENDPOINT: &str = "https://api.resend.com/emails";

/// Sends one HTML email through Resend. `to` is a single recipient address;
/// `from` and the API key come from [`MailConfig`].
pub async fn send(
    client: &reqwest::Client,
    cfg: &MailConfig,
    to: &str,
    subject: &str,
    html: &str,
) -> ApiResult<()> {
    let body = serde_json::json!({
        "from": cfg.from,
        "to": [to],
        "subject": subject,
        "html": html,
    });

    let response = client
        .post(RESEND_ENDPOINT)
        .bearer_auth(&cfg.resend_api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| ApiError::internal(format!("resend request failed: {e}")))?;

    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        Err(ApiError::internal(format!(
            "resend returned {status}: {detail}"
        )))
    }
}

/// Minimal, brand-consistent HTML email: a heading, a lead line, a big button,
/// a copy-paste URL fallback, and an "ignore this" footer. Shared by the reset
/// and verification mails so the two look like one family.
pub fn action_email(
    heading: &str,
    lead: &str,
    button_label: &str,
    url: &str,
    footer: &str,
) -> String {
    format!(
        r#"<!doctype html>
<html>
  <body style="margin:0;padding:0;background:#0b0d12;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;">
    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background:#0b0d12;padding:32px 0;">
      <tr>
        <td align="center">
          <table role="presentation" width="440" cellpadding="0" cellspacing="0" style="max-width:440px;width:100%;background:#141821;border:1px solid #232a36;border-radius:12px;overflow:hidden;">
            <tr>
              <td style="padding:32px 32px 8px 32px;">
                <div style="font-size:16px;font-weight:700;color:#e8eaed;letter-spacing:-0.01em;">Stake Dev Tool Cloud</div>
              </td>
            </tr>
            <tr>
              <td style="padding:8px 32px 0 32px;">
                <h1 style="margin:0 0 8px 0;font-size:20px;font-weight:600;color:#e8eaed;letter-spacing:-0.02em;">{heading}</h1>
                <p style="margin:0 0 24px 0;font-size:14px;line-height:1.6;color:#a3abb8;">{lead}</p>
                <a href="{url}" style="display:inline-block;background:#6366f1;color:#ffffff;text-decoration:none;font-size:14px;font-weight:600;padding:10px 20px;border-radius:8px;">{button_label}</a>
                <p style="margin:24px 0 4px 0;font-size:12px;color:#6b7280;">Or paste this link into your browser:</p>
                <p style="margin:0 0 24px 0;font-size:12px;word-break:break-all;"><a href="{url}" style="color:#818cf8;">{url}</a></p>
              </td>
            </tr>
            <tr>
              <td style="padding:0 32px 32px 32px;border-top:1px solid #232a36;">
                <p style="margin:16px 0 0 0;font-size:12px;line-height:1.6;color:#6b7280;">{footer}</p>
              </td>
            </tr>
          </table>
        </td>
      </tr>
    </table>
  </body>
</html>"#
    )
}
