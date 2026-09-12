use crate::math_engine::MathEngine;
use crate::saved_rounds::{SavedRoundsStore, default_store};
use crate::session::SessionStore;
use crate::settings::{self, SettingsStore};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForcedEvent {
    pub mode: String,
    #[serde(rename = "eventId")]
    pub event_id: u32,
}

pub struct AppState {
    pub sessions: Arc<SessionStore>,
    pub engine: Arc<MathEngine>,
    pub saved_rounds: Arc<SavedRoundsStore>,
    pub settings: Arc<SettingsStore>,
    /// When set, `/play` calls with matching `mode` bypass the RNG and return
    /// this exact event. Cleared via the `/api/devtool/force-event` endpoint.
    pub forced_event: Mutex<Option<ForcedEvent>>,
}

impl AppState {
    pub fn new(engine: MathEngine) -> Self {
        Self {
            sessions: Arc::new(SessionStore::new()),
            engine: Arc::new(engine),
            saved_rounds: default_store().expect("resolve local saved-rounds path"),
            settings: settings::default_store().expect("resolve local settings path"),
            forced_event: Mutex::new(None),
        }
    }

    pub fn from_parts(sessions: Arc<SessionStore>, engine: Arc<MathEngine>) -> Self {
        Self::from_parts_with_stores(
            sessions,
            engine,
            default_store().expect("resolve local saved-rounds path"),
            settings::default_store().expect("resolve local settings path"),
        )
    }

    pub fn from_parts_with_stores(
        sessions: Arc<SessionStore>,
        engine: Arc<MathEngine>,
        saved_rounds: Arc<SavedRoundsStore>,
        settings: Arc<SettingsStore>,
    ) -> Self {
        Self {
            sessions,
            engine,
            saved_rounds,
            settings,
            forced_event: Mutex::new(None),
        }
    }
}
