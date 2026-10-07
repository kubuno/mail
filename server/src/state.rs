use crate::config::Settings;
use kubuno_db::DbPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db:       DbPool,
    pub settings: Arc<Settings>,
}
