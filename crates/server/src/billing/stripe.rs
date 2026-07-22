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

/// The effective Stripe REST base. Production always talks to `api.stripe.com`;
/// the `STRIPE_API_BASE` env var overrides it so tests can point the client at a
/// local mock server (there is no host selector in [`StripeConfig`]).
fn api_base() -> String {
    std::env::var("STRIPE_API_BASE").unwrap_or_else(|_| STRIPE_API_BASE.to_string())
}

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

/// Creates a Stripe hosted Checkout Session (subscription mode) carrying every
/// `(price_id, quantity)` in `line_items` (e.g. a seat price plus, optionally, the
/// storage add-on), tagging both the session and the resulting subscription with
/// the workspace id (so the webhook can bind it) and the success/cancel URLs the
/// browser returns to. Returns the hosted checkout URL.
pub async fn create_checkout(
    client: &reqwest::Client,
    cfg: &StripeConfig,
    line_items: &[(&str, i64)],
    workspace_id: Uuid,
    success_url: &str,
) -> Result<String, ApiError> {
    let ws = workspace_id.to_string();
    let cancel_url = cancel_url_from(success_url);
    // Stripe expects application/x-www-form-urlencoded with bracketed nested keys.
    let mut form: Vec<(String, String)> = vec![
        ("mode".to_string(), "subscription".to_string()),
        (
            "subscription_data[metadata][workspace_id]".to_string(),
            ws.clone(),
        ),
        ("metadata[workspace_id]".to_string(), ws),
        ("success_url".to_string(), success_url.to_string()),
        ("cancel_url".to_string(), cancel_url),
    ];
    for (i, (price_id, quantity)) in line_items.iter().enumerate() {
        form.push((format!("line_items[{i}][price]"), price_id.to_string()));
        form.push((format!("line_items[{i}][quantity]"), quantity.to_string()));
    }

    let response = client
        .post(format!("{}/v1/checkout/sessions", api_base()))
        .bearer_auth(&cfg.secret_key)
        .form(&form)
        .send()
        .await
        .map_err(stripe_unreachable)?;

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

/// A subscription's seat line item: the Stripe subscription-item id (`si_…`, the
/// handle an update targets) and its current quantity (the seat count).
#[derive(Debug, Clone)]
pub struct SeatItem {
    pub id: String,
    pub quantity: i64,
}

// Minimal projections of the Stripe subscription object we read back: only the
// item id, quantity, and price id are needed to locate the seat line item.
#[derive(Deserialize)]
struct SubscriptionObject {
    items: SubscriptionItemList,
}
#[derive(Deserialize)]
struct SubscriptionItemList {
    data: Vec<SubscriptionItem>,
}
#[derive(Deserialize)]
struct SubscriptionItem {
    id: String,
    #[serde(default = "one")]
    quantity: i64,
    price: PriceRef,
}
#[derive(Deserialize)]
struct PriceRef {
    id: String,
}
fn one() -> i64 {
    1
}

/// A `502 stripe_unreachable` from a transport-level failure reaching Stripe.
fn stripe_unreachable(e: reqwest::Error) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "stripe_unreachable",
        format!("could not reach Stripe: {e}"),
    )
}

/// Fetches a subscription and returns its seat line item — the item whose price is
/// one of the two configured seat prices. `Ok(None)` when the subscription carries
/// no seat price (e.g. a storage-only subscription), so the caller can reject the
/// seat change cleanly rather than mutate the wrong item.
pub async fn fetch_seat_item(
    client: &reqwest::Client,
    cfg: &StripeConfig,
    subscription_id: &str,
) -> Result<Option<SeatItem>, ApiError> {
    let response = client
        .get(format!("{}/v1/subscriptions/{subscription_id}", api_base()))
        .bearer_auth(&cfg.secret_key)
        .send()
        .await
        .map_err(stripe_unreachable)?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(ApiError::internal(format!(
            "Stripe subscription fetch failed ({status}): {detail}"
        )));
    }

    let sub: SubscriptionObject = response
        .json()
        .await
        .map_err(|e| ApiError::internal(format!("malformed Stripe subscription response: {e}")))?;

    Ok(sub
        .items
        .data
        .into_iter()
        .find(|item| interval_for_price(cfg, &item.price.id).is_some())
        .map(|item| SeatItem {
            id: item.id,
            quantity: item.quantity,
        }))
}

/// Updates the seat line item's quantity on a subscription, prorating the change
/// (`proration_behavior=create_prorations`) so the customer is billed only the
/// difference on their next invoice.
pub async fn update_seat_quantity(
    client: &reqwest::Client,
    cfg: &StripeConfig,
    subscription_id: &str,
    item_id: &str,
    seats: i64,
) -> Result<(), ApiError> {
    let form: Vec<(String, String)> = vec![
        ("items[0][id]".to_string(), item_id.to_string()),
        ("items[0][quantity]".to_string(), seats.to_string()),
        (
            "proration_behavior".to_string(),
            "create_prorations".to_string(),
        ),
    ];

    let response = client
        .post(format!("{}/v1/subscriptions/{subscription_id}", api_base()))
        .bearer_auth(&cfg.secret_key)
        .form(&form)
        .send()
        .await
        .map_err(stripe_unreachable)?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(ApiError::internal(format!(
            "Stripe subscription update failed ({status}): {detail}"
        )));
    }
    Ok(())
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
