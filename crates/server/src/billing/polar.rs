//! The Polar REST client (checkout creation) and the product-id ↔ (plan,
//! interval) mapping shared by checkout and the webhook.

use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use protocol::billing::{BillingInterval, PlanId};

use crate::config::PolarConfig;
use crate::error::ApiError;

/// The configured product id for a (plan, interval) pair.
pub fn product_id_for(cfg: &PolarConfig, plan: PlanId, interval: BillingInterval) -> &str {
    match (plan, interval) {
        (PlanId::Solo, BillingInterval::Monthly) => &cfg.product_solo_monthly,
        (PlanId::Solo, BillingInterval::Yearly) => &cfg.product_solo_yearly,
        (PlanId::Team, BillingInterval::Monthly) => &cfg.product_team_monthly,
        (PlanId::Team, BillingInterval::Yearly) => &cfg.product_team_yearly,
    }
}

/// Reverse mapping: which (plan, interval) a Polar product id corresponds to, or
/// `None` if it is not one of the four configured products.
pub fn plan_for_product(cfg: &PolarConfig, product_id: &str) -> Option<(PlanId, BillingInterval)> {
    if product_id == cfg.product_solo_monthly {
        Some((PlanId::Solo, BillingInterval::Monthly))
    } else if product_id == cfg.product_solo_yearly {
        Some((PlanId::Solo, BillingInterval::Yearly))
    } else if product_id == cfg.product_team_monthly {
        Some((PlanId::Team, BillingInterval::Monthly))
    } else if product_id == cfg.product_team_yearly {
        Some((PlanId::Team, BillingInterval::Yearly))
    } else {
        None
    }
}

#[derive(Deserialize)]
struct CheckoutCreated {
    url: String,
}

/// Creates a Polar hosted checkout for `product_id`, tagging it with the
/// workspace id (so the webhook can bind the resulting subscription) and the
/// success URL the browser returns to. Returns the hosted checkout URL.
pub async fn create_checkout(
    client: &reqwest::Client,
    cfg: &PolarConfig,
    product_id: &str,
    workspace_id: Uuid,
    success_url: &str,
) -> Result<String, ApiError> {
    let body = serde_json::json!({
        "products": [product_id],
        "metadata": { "workspace_id": workspace_id.to_string() },
        "success_url": success_url,
    });

    let response = client
        .post(format!("{}/v1/checkouts", cfg.api_base()))
        .bearer_auth(&cfg.access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "polar_unreachable",
                format!("could not reach Polar: {e}"),
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(ApiError::internal(format!(
            "Polar checkout creation failed ({status}): {detail}"
        )));
    }

    let created: CheckoutCreated = response
        .json()
        .await
        .map_err(|e| ApiError::internal(format!("malformed Polar checkout response: {e}")))?;
    Ok(created.url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PolarServer;

    fn cfg() -> PolarConfig {
        PolarConfig {
            access_token: "tok".into(),
            webhook_secret: "whsec_x".into(),
            product_solo_monthly: "prod_solo_m".into(),
            product_solo_yearly: "prod_solo_y".into(),
            product_team_monthly: "prod_team_m".into(),
            product_team_yearly: "prod_team_y".into(),
            server: PolarServer::Production,
        }
    }

    #[test]
    fn product_mapping_round_trips() {
        let c = cfg();
        for (plan, interval, id) in [
            (PlanId::Solo, BillingInterval::Monthly, "prod_solo_m"),
            (PlanId::Solo, BillingInterval::Yearly, "prod_solo_y"),
            (PlanId::Team, BillingInterval::Monthly, "prod_team_m"),
            (PlanId::Team, BillingInterval::Yearly, "prod_team_y"),
        ] {
            assert_eq!(product_id_for(&c, plan, interval), id);
            assert_eq!(plan_for_product(&c, id), Some((plan, interval)));
        }
        assert_eq!(plan_for_product(&c, "prod_unknown"), None);
    }
}
