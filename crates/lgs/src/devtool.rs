use crate::config::intern_currency;
use crate::error::{AppError, AppResult};
use crate::saved_rounds;
use crate::session::SessionInit;
use crate::settings;
use crate::state::{AppState, ForcedEvent};
use crate::types::EventEntry;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/devtool/sessions", delete(reset_sessions))
        .route("/api/devtool/sessions/prepare", post(prepare_session))
        .route("/api/devtool/status", get(status))
        .route("/api/devtool/settings", get(get_settings_handler))
        .route(
            "/api/devtool/settings/toggle",
            post(toggle_resolution_handler),
        )
        .route(
            "/api/devtool/settings/custom",
            post(add_custom_resolution_handler),
        )
        .route(
            "/api/devtool/settings/custom/:id",
            delete(delete_custom_resolution_handler),
        )
        .route(
            "/api/devtool/force-event",
            get(get_forced_event)
                .post(set_forced_event)
                .delete(clear_forced_event),
        )
        .route("/api/devtool/sessions/:sid/last-event", get(get_last_event))
        .route("/api/devtool/sessions/:sid/events", get(get_events_history))
        .route("/api/devtool/sessions/:sid/stream", get(stream_events))
        .route(
            "/api/devtool/saved-rounds",
            get(list_saved_rounds).post(create_saved_round),
        )
        .route(
            "/api/devtool/saved-rounds/:id",
            patch(update_saved_round).delete(delete_saved_round),
        )
        .route("/api/devtool/bet-stats/:game", get(get_bet_stats))
        .route("/api/devtool/games/:game/modes", get(get_game_modes))
        .with_state(state)
}

#[derive(Deserialize)]
pub struct PrepareSessionBody {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    #[serde(default)]
    pub balance: Option<u64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Serialize)]
pub struct PrepareSessionResponse {
    pub ok: bool,
}

async fn prepare_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PrepareSessionBody>,
) -> AppResult<Json<PrepareSessionResponse>> {
    let init = SessionInit {
        game: body.game_slug,
        language: body.language,
        balance: body.balance,
        currency: body.currency.as_deref().map(intern_currency),
    };
    state.sessions.prepare(&body.session_id, init);
    Ok(Json(PrepareSessionResponse { ok: true }))
}

async fn reset_sessions(State(state): State<Arc<AppState>>) -> AppResult<Json<OkResponse>> {
    state.sessions.reset_all()?;
    Ok(Json(OkResponse { ok: true }))
}

#[derive(Serialize)]
struct StatusResponse {
    ok: bool,
    version: &'static str,
}

async fn status() -> Json<StatusResponse> {
    Json(StatusResponse {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
    })
}

// ========== Settings (resolutions) ==========

async fn get_settings_handler() -> AppResult<Json<settings::Settings>> {
    let s = settings::load().await?;
    Ok(Json(s))
}

#[derive(Deserialize)]
pub struct ToggleResolutionBody {
    pub id: String,
    pub enabled: bool,
}

async fn toggle_resolution_handler(
    Json(body): Json<ToggleResolutionBody>,
) -> AppResult<Json<settings::Settings>> {
    let s = settings::toggle(&body.id, body.enabled).await?;
    Ok(Json(s))
}

#[derive(Deserialize)]
pub struct AddCustomResolutionBody {
    pub label: String,
    pub width: u32,
    pub height: u32,
}

async fn add_custom_resolution_handler(
    Json(body): Json<AddCustomResolutionBody>,
) -> AppResult<Json<settings::Settings>> {
    let s = settings::add_custom(body.label, body.width, body.height).await?;
    Ok(Json(s))
}

async fn delete_custom_resolution_handler(
    Path(id): Path<String>,
) -> AppResult<Json<settings::Settings>> {
    let s = settings::delete_custom(&id).await?;
    Ok(Json(s))
}

// ========== Force event / replay debug helpers ==========

#[derive(Serialize)]
pub struct ForcedEventResponse {
    pub forced: Option<ForcedEvent>,
}

async fn get_forced_event(State(state): State<Arc<AppState>>) -> Json<ForcedEventResponse> {
    Json(ForcedEventResponse {
        forced: state.forced_event.lock().clone(),
    })
}

async fn set_forced_event(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ForcedEvent>,
) -> Json<ForcedEventResponse> {
    *state.forced_event.lock() = Some(body);
    Json(ForcedEventResponse {
        forced: state.forced_event.lock().clone(),
    })
}

async fn clear_forced_event(State(state): State<Arc<AppState>>) -> Json<ForcedEventResponse> {
    *state.forced_event.lock() = None;
    Json(ForcedEventResponse { forced: None })
}

#[derive(Serialize)]
pub struct LastEventResponse {
    #[serde(rename = "eventId")]
    pub event_id: Option<u32>,
    #[serde(rename = "payoutMultiplier")]
    pub payout_multiplier: Option<u32>,
}

async fn get_last_event(
    State(state): State<Arc<AppState>>,
    Path(sid): Path<String>,
) -> AppResult<Json<LastEventResponse>> {
    let s = state.sessions.get(&sid).ok_or(AppError::SessionNotFound)?;
    Ok(Json(LastEventResponse {
        event_id: s.last_event_id,
        payout_multiplier: s.last_payout_multiplier,
    }))
}

#[derive(Serialize)]
pub struct EventsHistoryResponse {
    pub count: usize,
    pub events: Vec<crate::types::EventEntry>,
}

async fn get_events_history(
    State(state): State<Arc<AppState>>,
    Path(sid): Path<String>,
) -> AppResult<Json<EventsHistoryResponse>> {
    let s = state.sessions.get(&sid).ok_or(AppError::SessionNotFound)?;
    Ok(Json(EventsHistoryResponse {
        count: s.event_history.len(),
        events: s.event_history,
    }))
}

/// SSE stream of new events for a session. On connect, emits a `snapshot`
/// event containing the current history (most-recent-first), then an `event`
/// for each subsequent push. Replaces per-frame polling of `/last-event` +
/// `/events` at 1 Hz — one persistent connection per frame, zero traffic
/// when no spin happens.
///
/// Always returns 200 OK, even if the session hasn't been prepared yet.
/// `EventSource` permanently closes on 4xx responses (no auto-reconnect on
/// HTTP errors, only on network-level failures), so the test view used to
/// silently lose its history feed when the SSE call beat `/sessions/prepare`.
/// We pre-create the broadcast channel; the snapshot is an empty list when
/// the session isn't there yet, and live events flow in once `/play` runs.
async fn stream_events(
    State(state): State<Arc<AppState>>,
    Path(sid): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe BEFORE reading the snapshot so any event pushed in the window
    // between the two lands in the live stream; client de-dupes on `at`.
    let rx = state.sessions.subscribe_events(&sid);
    let history = state
        .sessions
        .get(&sid)
        .map(|s| s.event_history)
        .unwrap_or_default();

    let snapshot_json = serde_json::to_string(&history).unwrap_or_else(|_| "[]".to_string());
    let snapshot_event = Event::default().event("snapshot").data(snapshot_json);
    let snapshot_stream = tokio_stream::once(Ok::<Event, Infallible>(snapshot_event));

    let live_stream = BroadcastStream::new(rx).filter_map(|r| match r {
        Ok(entry) => event_from_entry(&entry).map(Ok),
        // BroadcastStreamRecvError::Lagged: subscriber fell behind. Skip.
        Err(_) => None,
    });

    let combined = snapshot_stream.chain(live_stream);
    Sse::new(combined).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

fn event_from_entry(entry: &EventEntry) -> Option<Event> {
    serde_json::to_string(entry)
        .ok()
        .map(|j| Event::default().event("event").data(j))
}

// ========== Saved rounds ==========

#[derive(Deserialize)]
pub struct SavedRoundsQuery {
    #[serde(default, rename = "gameSlug")]
    pub game_slug: Option<String>,
}

#[derive(Serialize)]
pub struct SavedRoundsResponse {
    pub rounds: Vec<saved_rounds::SavedRound>,
}

async fn list_saved_rounds(
    Query(q): Query<SavedRoundsQuery>,
) -> AppResult<Json<SavedRoundsResponse>> {
    let rounds = saved_rounds::list(q.game_slug.as_deref()).await?;
    Ok(Json(SavedRoundsResponse { rounds }))
}

#[derive(Deserialize)]
pub struct CreateSavedRoundBody {
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    pub mode: String,
    #[serde(rename = "eventId")]
    pub event_id: u32,
    #[serde(default)]
    pub description: String,
}

async fn create_saved_round(
    Json(body): Json<CreateSavedRoundBody>,
) -> AppResult<Json<saved_rounds::SavedRound>> {
    let r =
        saved_rounds::create(body.game_slug, body.mode, body.event_id, body.description).await?;
    Ok(Json(r))
}

#[derive(Deserialize)]
pub struct UpdateSavedRoundBody {
    pub description: String,
}

async fn update_saved_round(
    Path(id): Path<String>,
    Json(body): Json<UpdateSavedRoundBody>,
) -> AppResult<Json<saved_rounds::SavedRound>> {
    let r = saved_rounds::update_description(&id, body.description).await?;
    Ok(Json(r))
}

#[derive(Serialize)]
pub struct OkResponse {
    pub ok: bool,
}

async fn delete_saved_round(Path(id): Path<String>) -> AppResult<Json<OkResponse>> {
    saved_rounds::delete(&id).await?;
    Ok(Json(OkResponse { ok: true }))
}

// ========== Notable bets per mode (for the test view's "Notable rounds" panel) ==========

#[derive(Serialize)]
pub struct BetStatsResponse {
    pub modes: Vec<crate::math_engine::ModeBetStats>,
}

async fn get_bet_stats(
    State(state): State<Arc<AppState>>,
    Path(game): Path<String>,
) -> AppResult<Json<BetStatsResponse>> {
    let modes = state.engine.game_bet_stats(&game).await?;
    Ok(Json(BetStatsResponse { modes }))
}

// ========== Mode list (for the test view dropdowns) ==========

#[derive(Serialize)]
pub struct GameModesResponse {
    pub modes: Vec<String>,
}

async fn get_game_modes(
    State(state): State<Arc<AppState>>,
    Path(game): Path<String>,
) -> AppResult<Json<GameModesResponse>> {
    let cfg = state.engine.load_config(&game).await?;
    let modes = cfg.modes.iter().map(|m| m.name.clone()).collect();
    Ok(Json(GameModesResponse { modes }))
}
