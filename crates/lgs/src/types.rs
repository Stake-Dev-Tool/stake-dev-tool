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

/// Fixed-point scale for a mode's cost multiplier. Bet arithmetic runs in
/// thousandths of a base bet so fractional buy prices (`1.25`, `2.5`) stay
/// exact and round-trip — plain f64 division would drift on large stakes.
pub const COST_SCALE: u64 = 1_000;

#[derive(Debug, Clone, Deserialize)]
pub struct GameMode {
    pub name: String,
    /// Cost multiplier of the mode: what one spin costs as a multiple of the
    /// base bet. math-sdk emits this as an integer (`1`, `100`), as an
    /// integer-valued float (`300.0`) or as a fractional buy/ante price
    /// (`2.5`, `1.25`) depending on the generator and the game, so the wire
    /// type is a decimal. Use [`GameMode::total_bet`] / [`GameMode::base_bet`]
    /// rather than multiplying by this directly.
    #[serde(deserialize_with = "de_cost")]
    pub cost: f64,
    pub events: String,
    pub weights: String,
}

impl GameMode {
    /// The cost multiplier in thousandths (`2.5` → `2500`). Exact: `de_cost`
    /// rejects any value that isn't representable at this scale.
    pub fn cost_milli(&self) -> u64 {
        let scaled = (self.cost * COST_SCALE as f64).round();
        if scaled.is_finite() && scaled >= 1.0 {
            scaled as u64
        } else {
            COST_SCALE
        }
    }

    /// What a `base_bet` actually costs the player in this mode.
    pub fn total_bet(&self, base_bet: u64) -> u64 {
        let total = u128::from(base_bet) * u128::from(self.cost_milli()) / u128::from(COST_SCALE);
        u64::try_from(total).unwrap_or(u64::MAX)
    }

    /// Inverse of [`GameMode::total_bet`]: the base bet a total stake buys.
    /// Payouts are quoted against the base bet, not against what was staked.
    pub fn base_bet(&self, total_bet: u64) -> u64 {
        let base = u128::from(total_bet) * u128::from(COST_SCALE) / u128::from(self.cost_milli());
        u64::try_from(base).unwrap_or(u64::MAX)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CostValue {
    Integer(u64),
    Float(f64),
}

fn de_cost<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let value = match CostValue::deserialize(d)? {
        CostValue::Integer(value) => value as f64,
        CostValue::Float(value) => value,
    };
    // Anything the fixed-point bet arithmetic couldn't represent exactly is an
    // error rather than a silent rounding of the player's stake.
    let scaled = value * COST_SCALE as f64;
    if value.is_finite() && value > 0.0 && scaled.fract() == 0.0 && scaled <= u64::MAX as f64 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "invalid mode cost {value}; expected a positive number with at most 3 decimals (e.g. 1, 2.5, 100)"
        )))
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
        assert_eq!(mode_with_cost("300").expect("integer cost").cost, 300.0);
        assert_eq!(
            mode_with_cost("300.0")
                .expect("integer-valued float cost")
                .cost,
            300.0
        );
    }

    #[test]
    fn mode_cost_accepts_fractional_buy_prices() {
        for (json, expected) in [("2.5", 2.5), ("1.25", 1.25), ("0.5", 0.5)] {
            let mode = mode_with_cost(json).unwrap_or_else(|e| panic!("cost {json}: {e}"));
            assert_eq!(mode.cost, expected);
        }
        assert_eq!(mode_with_cost("2.5").expect("2.5").cost_milli(), 2500);
    }

    #[test]
    fn mode_cost_rejects_values_that_would_be_silently_changed() {
        // Below the thousandth — the fixed-point bet arithmetic can't hold it.
        assert!(mode_with_cost("2.5001").is_err());
        assert!(mode_with_cost("0").is_err());
        assert!(mode_with_cost("-1").is_err());
    }

    #[test]
    fn fractional_cost_bet_arithmetic_round_trips() {
        let mode = mode_with_cost("2.5").expect("fractional cost");
        // One unit at API scale: 2.5× costs 2.5 units, and the payout base is
        // the original bet again.
        assert_eq!(mode.total_bet(API_MULTIPLIER), 2_500_000);
        assert_eq!(mode.base_bet(2_500_000), API_MULTIPLIER);

        let base = mode_with_cost("1").expect("base cost");
        assert_eq!(base.total_bet(API_MULTIPLIER), API_MULTIPLIER);
        assert_eq!(base.base_bet(API_MULTIPLIER), API_MULTIPLIER);
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
