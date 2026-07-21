use lgs::state::AppState as LgsState;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;

pub struct LgsRunning {
    pub bound_addr: SocketAddr,
    pub math_dir: String,
    pub state: Arc<LgsState>,
    pub shutdown: oneshot::Sender<()>,
    pub join: tokio::task::JoinHandle<std::io::Result<()>>,
}

#[derive(Default)]
pub struct AppState {
    pub running: Mutex<Option<LgsRunning>>,
    /// The live workspace SSE subscription task, if any. Held so switching the
    /// active workspace can abort the previous stream before starting a new one.
    pub cloud_sse: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
}
