use crate::error::AppResult;
use crate::state::AppState;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::get;
use serde::Serialize;
use serde_json::value::RawValue;
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // Canonical Stake Engine replay path — mounted at the LGS root so the
        // game's `{rgs_url}/bet/replay/...` fetch lands here.
        .route(
            "/bet/replay/:game/:version/:mode/:event",
            get(replay_handler),
        )
        .with_state(state)
}

#[derive(Serialize)]
pub struct ReplayResponse {
    #[serde(rename = "payoutMultiplier")]
    pub payout_multiplier: f64,
    #[serde(rename = "costMultiplier")]
    pub cost_multiplier: f64,
    pub state: Arc<RawValue>,
}

async fn replay_handler(
    State(state): State<Arc<AppState>>,
    Path((game, _version, mode, event)): Path<(String, String, String, u32)>,
) -> AppResult<Json<ReplayResponse>> {
    // `version` is accepted for protocol compatibility but unused — our LGS
    // serves a single math version per game folder.
    let result = state.engine.replay_event(&game, &mode, event).await?;
    Ok(Json(ReplayResponse {
        payout_multiplier: result.payout_multiplier as f64 / 100.0,
        cost_multiplier: result.cost_multiplier,
        state: result.state,
    }))
}
