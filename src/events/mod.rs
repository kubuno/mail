use anyhow::Result;
use reqwest::Client;
use serde_json::Value;

use crate::config::Settings;

pub async fn publish_event(http: &Client, settings: &Settings, event_type: &str, payload: Value) -> Result<()> {
    let url    = format!("{}/internal/events/publish", settings.core.url);
    let secret = &settings.core.internal_secret;

    http.post(&url)
        .header("X-Internal-Secret", secret.as_str())
        .json(&serde_json::json!({
            "type":    event_type,
            "module":  "mail",
            "payload": payload,
        }))
        .send()
        .await?;

    Ok(())
}
