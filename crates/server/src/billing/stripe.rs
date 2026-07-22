//! The Stripe REST client (checkout session creation) and the price-id ↔ (plan,
//! interval) mapping shared by checkout and the webhook.

use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use protocol::billing::BillingInterval;

use crate::config::StripeConfig;
use crate::error::ApiError;

/// Stripe's REST API base. Test vs live is chosen by the secret key, not the URL.
const STRIPE_API_BASE: &str = "https://api.stripe.com";

/// The configured per-seat price id for an interval. The subscription quantity is
/// the seat count; Stripe's graduated tiers compute €3 first seat + €2 each more.
pub fn seat_price_id(cfg: &StripeConfig, interval: BillingInterval) -> &str {
    match interval {
        BillingInterval::Monthly => &cfg.price_seat_monthly,
        BillingInterval::Yearly => &cfg.price_seat_yearly,
    }
}

/// Reverse mapping: which interval a Stripe price id is the seat price for, or
/// `None` if it is not one of the two configured seat prices (the storage price is
/// deliberately excluded — it maps to no plan).
pub fn interval_for_price(cfg: &StripeConfig, price_id: &str) -> Option<BillingInterval> {
    if price_id == cfg.price_seat_monthly {
        Some(BillingInterval::Monthly)
    } else if price_id == cfg.price_seat_yearly {
        Some(BillingInterval::Yearly)
    } else {
        None
    }
}

/// The cancel URL for a checkout: the success URL with its query string dropped
/// (removing the `?upgraded=1` success flag), so a canceled checkout returns the
/// user to the plain workspace page.
fn cancel_url_from(success_url: &str) -> String {
    match success_url.split_once('?') {
        Some((base, _)) => base.to_string(),
        None => success_url.to_string(),
    }
}

#[derive(Deserialize)]
struct CheckoutCreated {
    url: String,
}

/// Creates a Stripe hosted Checkout Session (subscription mode) for `price_id`
/// with `quantity`, tagging both the session and the resulting subscription with
/// the workspace id (so the webhook can bind it) and the success/cancel URLs the
/// browser returns to. Returns the hosted checkout URL.
pub async fn create_checkout(
    client: &reqwest::Client,
    cfg: &StripeConfig,
    price_id: &str,
    workspace_id: Uuid,
    success_url: &str,
    quantity: i64,
) -> Result<String, ApiError> {
    let ws = workspace_id.to_string();
    let cancel_url = cancel_url_from(success_url);
    // Stripe expects application/x-www-form-urlencoded with bracketed nested keys.
    let form: Vec<(&str, String)> = vec![
        ("mode", "subscription".to_string()),
        ("line_items[0][price]", price_id.to_string()),
        ("line_items[0][quantity]", quantity.to_string()),
        ("subscription_data[metadata][workspace_id]", ws.clone()),
        ("metadata[workspace_id]", ws),
        ("success_url", success_url.to_string()),
        ("cancel_url", cancel_url),
    ];

    let response = client
        .post(format!("{STRIPE_API_BASE}/v1/checkout/sessions"))
        .bearer_auth(&cfg.secret_key)
        .form(&form)
        .send()
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "stripe_unreachable",
                format!("could not reach Stripe: {e}"),
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(ApiError::internal(format!(
            "Stripe checkout creation failed ({status}): {detail}"
        )));
    }

    let created: CheckoutCreated = response
        .json()
        .await
        .map_err(|e| ApiError::internal(format!("malformed Stripe checkout response: {e}")))?;
    Ok(created.url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> StripeConfig {
        StripeConfig {
            secret_key: "sk_test_x".into(),
            webhook_secret: "whsec_x".into(),
            price_seat_monthly: "price_seat_m".into(),
            price_seat_yearly: "price_seat_y".into(),
            price_storage: "price_storage".into(),
        }
    }

    #[test]
    fn price_mapping_round_trips() {
        let c = cfg();
        for (interval, id) in [
            (BillingInterval::Monthly, "price_seat_m"),
            (BillingInterval::Yearly, "price_seat_y"),
        ] {
            assert_eq!(seat_price_id(&c, interval), id);
            assert_eq!(interval_for_price(&c, id), Some(interval));
        }
        // The storage price maps to no plan, and neither does an unknown id.
        assert_eq!(interval_for_price(&c, "price_storage"), None);
        assert_eq!(interval_for_price(&c, "price_unknown"), None);
    }

    #[test]
    fn cancel_url_drops_the_upgraded_flag() {
        assert_eq!(
            cancel_url_from("https://app.example.com/w/acme?upgraded=1"),
            "https://app.example.com/w/acme"
        );
        // No query string → returned unchanged.
        assert_eq!(
            cancel_url_from("https://app.example.com/w/acme"),
            "https://app.example.com/w/acme"
        );
    }
}
