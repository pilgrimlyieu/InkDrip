use std::sync::Arc;

use inkdrip_core::config::AppConfig;
use inkdrip_core::store::BookStore;

/// Shared application state passed to all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub store: Arc<dyn BookStore>,
}
