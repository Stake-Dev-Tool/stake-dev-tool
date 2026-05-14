use crate::config;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::types::{
    AuthenticateResponse, Balance, BalanceResponse, BetEventResponse, EndRoundResponse, EventEntry,
    PlayResponse, Round,
};
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/rgs/:game/wallet/authenticate", post(authenticate))
        .route("/api/rgs/:game/wallet/balance", post(balance))
        .route("/api/rgs/:game/wallet/play", post(play))
        .route("/api/rgs/:game/wallet/end-round", post(end_round))
        .route("/api/rgs/:game/bet/event", post(bet_event))
        .with_state(state)
}

#[derive(Deserialize)]
struct AuthBody {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
    language: Option<String>,
}

async fn authenticate(
    State(state): State<Arc<AppState>>,
    Path(game): Path<String>,
    Json(body): Json<AuthBody>,
) -> AppResult<Json<AuthenticateResponse>> {
    let session_id = body.session_id.ok_or(AppError::MissingField("sessionID"))?;
    let session = state
        .sessions
        .get_or_create(&session_id, &game, body.language);

    let engine = state.engine.clone();
    let game_clone = game.clone();
    tokio::spawn(async move {
        if let Err(e) = engine.preload(&game_clone).await {
            tracing::warn!(?e, game = %game_clone, "preload failed");
        }
    });

    Ok(Json(AuthenticateResponse {
        balance: Balance {
            amount: session.balance,
            currency: session.currency,
        },
        round: session.active_round,
        config: config::auth_config(),
        meta: None,
    }))
}

#[derive(Deserialize)]
struct BalanceBody {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
}

async fn balance(
    State(state): State<Arc<AppState>>,
    Path(_game): Path<String>,
    Json(body): Json<BalanceBody>,
) -> AppResult<Json<BalanceResponse>> {
    let session_id = body.session_id.ok_or(AppError::MissingField("sessionID"))?;
    let session = state
        .sessions
        .get(&session_id)
        .ok_or(AppError::SessionNotFound)?;
    Ok(Json(BalanceResponse {
        balance: Balance {
            amount: session.balance,
            currency: session.currency,
        },
    }))
}

#[derive(Deserialize)]
struct PlayBody {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
    mode: Option<String>,
    amount: Option<u64>,
}

async fn play(
    State(state): State<Arc<AppState>>,
    Path(game): Path<String>,
    Json(body): Json<PlayBody>,
) -> AppResult<Json<PlayResponse>> {
    let session_id = body.session_id.ok_or(AppError::MissingField("sessionID"))?;
    let mode = body.mode.ok_or(AppError::MissingField("mode"))?;
    let amount = body.amount.ok_or(AppError::MissingField("amount"))?;

    let existing = state
        .sessions
        .get(&session_id)
        .ok_or(AppError::SessionNotFound)?;

    // Safety: if a previous round is still active (client called /play twice
    // without /end-round), credit its pending payout before taking the new
    // bet — otherwise the winnings would be lost when the active_round slot
    // is overwritten below.
    if let Some(round) = existing.active_round.as_ref()
        && round.payout > 0
    {
        state.sessions.add_winnings(&session_id, round.payout);
    }

    let mode_cost = state.engine.get_mode_cost(&game, &mode).await?;
    let total_cost = amount.saturating_mul(mode_cost);

    state
        .sessions
        .deduct_bet(&session_id, total_cost)
        .ok_or(AppError::InsufficientBalance)?;

    // If a forced event is set for this mode, bypass the RNG and return it.
    let forced = state
        .forced_event
        .lock()
        .as_ref()
        .filter(|f| f.mode == mode)
        .cloned();

    let result = if let Some(f) = forced {
        state
            .engine
            .play_forced(&game, &mode, total_cost, f.event_id)
            .await?
    } else {
        state.engine.play_spin(&game, &mode, total_cost).await?
    };

    // Winnings are NOT credited here — the RGS contract credits payouts at
    // /end-round, so the balance returned by /play reflects only the bet
    // deduction. See end_round() below.
    state
        .sessions
        .set_last_event(&session_id, result.event_id, result.payout_multiplier);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let forced_flag = state
        .forced_event
        .lock()
        .as_ref()
        .is_some_and(|f| f.mode == mode);
    state.sessions.push_event(
        &session_id,
        EventEntry {
            event_id: result.event_id,
            mode: mode.clone(),
            bet_amount: total_cost,
            payout: result.payout,
            payout_multiplier: result.payout_multiplier,
            forced: forced_flag,
            at: now_ms,
        },
    );

    let round = Round {
        bet_id: state.sessions.next_bet_id(),
        // The RGS charges bonus-buy modes by `amount * mode_cost`, but the
        // round contract reports the player's selected base stake. This keeps
        // resumed `/authenticate` rounds from reopening bonus buys at the
        // inflated charged amount.
        amount,
        payout: result.payout,
        payout_multiplier: result.payout_multiplier as f64 / 100.0,
        // Active until the client calls /end-round, which credits the payout.
        active: true,
        mode,
        event: Some(result.event_id.to_string()),
        state: result.state,
    };

    state
        .sessions
        .set_active_round(&session_id, Some(round.clone()));

    let final_session = state
        .sessions
        .get(&session_id)
        .ok_or(AppError::SessionNotFound)?;

    Ok(Json(PlayResponse {
        balance: Balance {
            amount: final_session.balance,
            currency: final_session.currency,
        },
        round,
    }))
}

#[derive(Deserialize)]
struct EndRoundBody {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
}

async fn end_round(
    State(state): State<Arc<AppState>>,
    Path(_game): Path<String>,
    Json(body): Json<EndRoundBody>,
) -> AppResult<Json<EndRoundResponse>> {
    let session_id = body.session_id.ok_or(AppError::MissingField("sessionID"))?;
    let session = state
        .sessions
        .get(&session_id)
        .ok_or(AppError::SessionNotFound)?;

    // Credit the pending round's payout now. /play only deducts the bet and
    // stores the outcome on active_round; we pay it out here so the client's
    // balance animation (spin → credit) matches the Stake Engine RGS contract.
    if let Some(round) = session.active_round.as_ref()
        && round.payout > 0
    {
        state.sessions.add_winnings(&session_id, round.payout);
    }
    state.sessions.set_active_round(&session_id, None);

    let session = state
        .sessions
        .get(&session_id)
        .ok_or(AppError::SessionNotFound)?;
    Ok(Json(EndRoundResponse {
        balance: Balance {
            amount: session.balance,
            currency: session.currency,
        },
        round: None,
        config: config::auth_config(),
        meta: None,
    }))
}

#[derive(Deserialize)]
struct BetEventBody {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
    event: Option<String>,
}

async fn bet_event(
    State(state): State<Arc<AppState>>,
    Path(_game): Path<String>,
    Json(body): Json<BetEventBody>,
) -> AppResult<Json<BetEventResponse>> {
    let session_id = body.session_id.ok_or(AppError::MissingField("sessionID"))?;
    state
        .sessions
        .get(&session_id)
        .ok_or(AppError::SessionNotFound)?;
    Ok(Json(BetEventResponse { event: body.event }))
}
