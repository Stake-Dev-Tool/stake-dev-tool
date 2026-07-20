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
    /// accept integer-valued forms without silently rounding invalid costs.
    #[serde(deserialize_with = "de_cost")]
    pub cost: u64,
    pub events: String,
    pub weights: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CostValue {
    Integer(u64),
    Float(f64),
}

fn de_cost<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    match CostValue::deserialize(d)? {
        CostValue::Integer(value) if value > 0 => Ok(value),
        CostValue::Float(value)
            if value.is_finite()
                && value >= 1.0
                && value.fract() == 0.0
                && value < u64::MAX as f64 =>
        {
            Ok(value as u64)
        }
        CostValue::Integer(value) => Err(serde::de::Error::custom(format!(
            "invalid mode cost {value}; expected a positive integer"
        ))),
        CostValue::Float(value) => Err(serde::de::Error::custom(format!(
            "invalid mode cost {value}; expected a positive integer"
        ))),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_with_cost(cost: &str) -> Result<GameMode, sonic_rs::Error> {
        sonic_rs::from_str(&format!(
            r#"{{"name":"base","cost":{cost},"events":"books.zst","weights":"weights.csv"}}"#
        ))
    }

    #[test]
    fn mode_cost_accepts_integer_and_integer_valued_float() {
        assert_eq!(mode_with_cost("300").expect("integer cost").cost, 300);
        assert_eq!(
            mode_with_cost("300.0")
                .expect("integer-valued float cost")
                .cost,
            300
        );
    }

    #[test]
    fn mode_cost_rejects_values_that_would_be_silently_changed() {
        assert!(mode_with_cost("300.5").is_err());
        assert!(mode_with_cost("0").is_err());
        assert!(mode_with_cost("-1").is_err());
    }
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
