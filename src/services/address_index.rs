// Recipient-autocomplete index maintenance. Called from the IMAP sync (weight 1)
// and from outgoing sends (higher weight: people the user writes to should rank
// first, like Gmail). Failures are logged but never block mail processing.
use sqlx::PgPool;
use uuid::Uuid;

/// Upsert a batch of (email, name) pairs into `mail.address_index`.
pub async fn upsert(db: &PgPool, user_id: Uuid, entries: &[(String, Option<String>)], weight: i32) {
    for (email, name) in entries {
        let email = email.trim();
        if !email.contains('@') || email.len() > 320 {
            continue;
        }
        let res = sqlx::query(
            r#"INSERT INTO mail.address_index (user_id, email, name, use_count, last_used_at)
               VALUES ($1, LOWER($2), NULLIF($3, ''), $4, NOW())
               ON CONFLICT (user_id, email) DO UPDATE SET
                   use_count    = mail.address_index.use_count + $4,
                   last_used_at = NOW(),
                   name         = COALESCE(NULLIF($3, ''), mail.address_index.name)"#,
        )
        .bind(user_id)
        .bind(email)
        .bind(name.as_deref().unwrap_or(""))
        .bind(weight)
        .execute(db)
        .await;
        if let Err(e) = res {
            tracing::error!(email, error = %e, "MAJ index d'adresses échouée");
        }
    }
}

/// Extract (email, name) pairs from a JSONB array of `{email, name}` objects.
pub fn from_json_list(v: &serde_json::Value) -> Vec<(String, Option<String>)> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let email = a.get("email")?.as_str()?.to_string();
                    let name = a.get("name").and_then(|n| n.as_str()).map(str::to_string);
                    Some((email, name))
                })
                .collect()
        })
        .unwrap_or_default()
}
