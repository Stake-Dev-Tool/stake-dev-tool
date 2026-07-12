use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::sync::Arc;

pub const API_MULTIPLIER: u64 = 1_000_000;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Session {
    pub id: String,
    pub game: String,
    pub balance: u64,
    pub currency: &'static str,
    pub language: String,
    pub active_round: Option<Round>,
    pub created_at: u64,
    /// Set after each `/play` so the test view can display "last event: #N".
    pub last_event_id: Option<u32>,
    pub last_payout_multiplier: Option<u32>,
    /// Ring-buffer-ish event history (most recent first, capped).
    pub event_history: Vec<EventEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEntry {
    #[serde(rename = "eventId")]
    pub event_id: u32,
    pub mode: String,
    #[serde(rename = "betAmount")]
    pub bet_amount: u64,
    pub payout: u64,
    #[serde(rename = "payoutMultiplier")]
    pub payout_multiplier: u32,
    #[serde(rename = "forced")]
    pub forced: bool,
    /// Unix ms timestamp.
    pub at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Round {
    #[serde(rename = "betID")]
    pub bet_id: u64,
    pub amount: u64,
    pub payout: u64,
    #[serde(rename = "payoutMultiplier")]
    pub payout_multiplier: f64,
    pub active: bool,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    pub state: Arc<RawValue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameMode {
    pub name: String,
    /// Cost multiplier of the mode. math-sdk emits this either as an integer
    /// or as a float (`"cost": 300.0`) depending on the generator version, so
    /// accept both and round to the RGS's integer cost model.
    #[serde(deserialize_with = "de_cost")]
    pub cost: u64,
    pub events: String,
    pub weights: String,
}

fn de_cost<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let raw = f64::deserialize(d)?;
    if !raw.is_finite() || raw < 0.0 {
        return Err(serde::de::Error::custom(format!("invalid mode cost {raw}")));
    }
    Ok((raw.round() as u64).max(1))
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameConfig {
    pub modes: Vec<GameMode>,
}

#[derive(Debug, Clone, Copy)]
pub struct WeightEntry {
    pub event_id: u32,
    pub weight: u64,
    pub payout_multiplier: u32,
}

#[derive(Debug, Serialize)]
pub struct Balance {
    pub amount: u64,
    pub currency: &'static str,
}

#[derive(Debug, Serialize)]
pub struct JurisdictionFlags {
    #[serde(rename = "socialCasino")]
    pub social_casino: bool,
    #[serde(rename = "disabledFullscreen")]
    pub disabled_fullscreen: bool,
    #[serde(rename = "disabledTurbo")]
    pub disabled_turbo: bool,
    #[serde(rename = "disabledSuperTurbo")]
    pub disabled_super_turbo: bool,
    #[serde(rename = "disabledAutoplay")]
    pub disabled_autoplay: bool,
    #[serde(rename = "disabledSlamstop")]
    pub disabled_slamstop: bool,
    #[serde(rename = "disabledSpacebar")]
    pub disabled_spacebar: bool,
    #[serde(rename = "disabledBuyFeature")]
    pub disabled_buy_feature: bool,
    #[serde(rename = "displayNetPosition")]
    pub display_net_position: bool,
    #[serde(rename = "displayRTP")]
    pub display_rtp: bool,
    #[serde(rename = "displaySessionTimer")]
    pub display_session_timer: bool,
    #[serde(rename = "minimumRoundDuration")]
    pub minimum_round_duration: u32,
}

#[derive(Debug, Serialize)]
pub struct AuthConfig {
    #[serde(rename = "gameID")]
    pub game_id: String,
    #[serde(rename = "minBet")]
    pub min_bet: u64,
    #[serde(rename = "maxBet")]
    pub max_bet: u64,
    #[serde(rename = "stepBet")]
    pub step_bet: u64,
    #[serde(rename = "defaultBetLevel")]
    pub default_bet_level: u64,
    #[serde(rename = "betLevels")]
    pub bet_levels: &'static [u64],
    #[serde(rename = "betModes")]
    pub bet_modes: serde_json::Value,
    pub jurisdiction: JurisdictionFlags,
}

#[derive(Debug, Serialize)]
pub struct AuthenticateResponse {
    pub balance: Balance,
    pub round: Option<Round>,
    pub config: AuthConfig,
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    pub balance: Balance,
}

#[derive(Debug, Serialize)]
pub struct PlayResponse {
    pub balance: Balance,
    pub round: Round,
}

#[derive(Debug, Serialize)]
pub struct EndRoundResponse {
    pub balance: Balance,
    pub round: Option<Round>,
    pub config: AuthConfig,
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct BetEventResponse {
    pub event: Option<String>,
}
