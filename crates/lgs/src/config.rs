use crate::types::{API_MULTIPLIER, AuthConfig, JurisdictionFlags};
use std::env;

pub const CURRENCY: &str = "USD";
pub const LANGUAGE: &str = "en";
pub const INITIAL_BALANCE: u64 = 10_000 * API_MULTIPLIER;

pub const SUPPORTED_CURRENCIES: &[&str] = &[
    "USD", "CAD", "JPY", "EUR", "RUB", "CNY", "PHP", "INR", "IDR", "KRW", "BRL", "MXN", "DKK",
    "PLN", "VND", "TRY", "CLP", "ARS", "PEN", "NGN", "SAR", "ILS", "AED", "TWD", "NOK", "KWD",
    "JOD", "CRC", "TND", "SGD", "MYR", "OMR", "QAR", "BHD", "XGC", "XSC", "XEC",
];

pub fn intern_currency(c: &str) -> &'static str {
    SUPPORTED_CURRENCIES
        .iter()
        .copied()
        .find(|s| s.eq_ignore_ascii_case(c))
        .unwrap_or(CURRENCY)
}

pub const MIN_BET: u64 = 10_000;
pub const MAX_BET: u64 = 2_000_000_000;
pub const STEP_BET: u64 = 10_000;
pub const DEFAULT_BET_LEVEL: u64 = 200_000;

pub const BET_LEVELS: &[u64] = &[
    10_000,
    20_000,
    50_000,
    100_000,
    200_000,
    400_000,
    600_000,
    800_000,
    1_000_000,
    1_200_000,
    1_400_000,
    1_600_000,
    1_800_000,
    2_000_000,
    3_000_000,
    4_000_000,
    5_000_000,
    6_000_000,
    7_000_000,
    8_000_000,
    9_000_000,
    10_000_000,
    12_000_000,
    14_000_000,
    16_000_000,
    18_000_000,
    20_000_000,
    30_000_000,
    40_000_000,
    50_000_000,
    75_000_000,
    100_000_000,
    150_000_000,
    200_000_000,
    250_000_000,
    300_000_000,
    350_000_000,
    400_000_000,
    450_000_000,
    500_000_000,
    750_000_000,
    1_000_000_000,
    1_250_000_000,
    1_500_000_000,
    2_000_000_000,
];

pub fn jurisdiction() -> JurisdictionFlags {
    JurisdictionFlags {
        social_casino: false,
        disabled_fullscreen: false,
        disabled_turbo: false,
        disabled_super_turbo: false,
        disabled_autoplay: false,
        disabled_slamstop: false,
        disabled_spacebar: false,
        disabled_buy_feature: false,
        display_net_position: false,
        display_rtp: true,
        display_session_timer: false,
        minimum_round_duration: 0,
    }
}

pub fn auth_config() -> AuthConfig {
    AuthConfig {
        game_id: String::new(),
        min_bet: MIN_BET,
        max_bet: MAX_BET,
        step_bet: STEP_BET,
        default_bet_level: DEFAULT_BET_LEVEL,
        bet_levels: BET_LEVELS,
        bet_modes: serde_json::json!({}),
        jurisdiction: jurisdiction(),
    }
}

pub struct ServerConfig {
    pub bind_addr: String,
    pub math_dir: String,
    pub ui_dir: Option<std::path::PathBuf>,
}

impl ServerConfig {
    pub fn from_env() -> Self {
        Self {
            bind_addr: env::var("LGS_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3001".to_string()),
            math_dir: env::var("LGS_MATH_DIR").unwrap_or_else(|_| "./math".to_string()),
            ui_dir: env::var("LGS_UI_DIR").ok().map(std::path::PathBuf::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_regional_social_currencies() {
        assert_eq!(intern_currency("XSC"), "XSC");
        assert_eq!(intern_currency("xec"), "XEC");
    }
}
