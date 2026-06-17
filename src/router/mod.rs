use axum::{
    routing::{delete, get, patch, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};

use crate::{
    handlers::{accounts, drafts, filters, labels, messages, spam, threads},
    state::AppState,
};

pub fn build(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Accounts
        .route("/accounts",              get(accounts::list_accounts).post(accounts::create_account))
        .route("/accounts/test",         post(accounts::test_connection))
        .route("/accounts/:id",          get(accounts::get_account).patch(accounts::update_account).delete(accounts::delete_account))
        .route("/accounts/:id/test",     post(accounts::test_existing_account))
        .route("/accounts/:id/sync",     post(accounts::trigger_sync))
        // Threads
        .route("/threads",               get(threads::list_threads))
        .route("/counts",                get(threads::counts))
        .route("/threads/:id",           get(threads::get_thread).delete(threads::delete_thread))
        .route("/threads/:id/star",      post(threads::star_thread))
        .route("/threads/:id/important", post(threads::important_thread))
        .route("/threads/:id/snooze",    post(threads::snooze_thread))
        .route("/threads/:id/read",      post(threads::read_thread))
        .route("/threads/:id/mute",      post(threads::mute_thread))
        .route("/threads/:id/move",      post(threads::move_thread))
        .route("/threads/:id/labels/:label_id", post(labels::add_thread_label).delete(labels::remove_thread_label))
        .route("/subscriptions",         get(threads::subscriptions))
        // Messages
        .route("/messages/:id",          get(messages::get_message).delete(messages::delete_message))
        .route("/messages/:id/attachments/:index", get(messages::download_attachment))
        .route("/messages/:id/star",     post(messages::star_message))
        .route("/messages/:id/read",     patch(messages::mark_read))
        .route("/send",                  post(messages::send_message))
        // Drafts
        .route("/drafts",                get(drafts::list_drafts).post(drafts::save_draft))
        .route("/scheduled",             get(drafts::scheduled_drafts))
        .route("/drafts/:id",            patch(drafts::update_draft).delete(drafts::delete_draft))
        // Labels
        .route("/labels",                get(labels::list_labels).post(labels::create_label))
        .route("/labels/:id",            delete(labels::delete_label))
        // Filtres / règles automatiques
        .route("/filters",               get(filters::list_filters).post(filters::create_filter))
        .route("/filters/:id",           delete(filters::delete_filter))
        // Adresses bloquées
        .route("/blocked",               get(filters::list_blocked).post(filters::block_sender))
        .route("/blocked/:id",           delete(filters::unblock_sender))
        // Anti-spam bayésien
        .route("/spam/stats",            get(spam::stats))
        .route("/spam/settings",         patch(spam::update_settings))
        .route("/spam/train",            post(spam::train))
        // Settings (renvoie vers la page settings du frontend)
        .route("/settings",              get(|| async { axum::Json(serde_json::json!({ "module": "mail" })) }))
        .layer(cors)
        .with_state(state)
}
