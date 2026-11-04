//! HTTP API, auth guard, metrics and the embedded UI (port of `internal/server`).
use axum::Router;

use crate::store::Store;

pub struct AppState {
    pub store: Store,
}

pub fn router(_state: AppState) -> Router {
    Router::new()
}
