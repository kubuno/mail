// Recipient-autocomplete index maintenance. Called from the IMAP sync (weight 1)
// and from outgoing sends (higher weight: people the user writes to should rank
// first, like Gmail). Failures are logged but never block mail processing.
use kubuno_db::DbPool;
use uuid::Uuid;

/// Upsert a batch of (email, name) pairs into `mail.address_index`.
pub async fn upsert(db: &DbPool, user_id: Uuid, entries: &[(String, Option<String>)], weight: i32) {
    for (email, name) in entries {
        let email = email.trim();
        if !email.contains('@') || email.len() > 320 {
            continue;
        }
        use kubuno_db::dialect::Assign;
        let upsert = db.backend().upsert(
            "mail.address_index",
            &["user_id", "email"],
            &[
                Assign::Expr { col: "use_count", expr: "{cur} + {new}" },
                Assign::Incoming("last_used_at"),
                Assign::Expr { col: "name", expr: "COALESCE({new}, {cur})" },
            ],
        );
        // Lowercasing and the empty-name-is-NULL rule are applied here, so the
        // conflict branch can refer to the incoming row's values directly.
        let name = name.as_deref().filter(|n| !n.is_empty());
        let res = crate::db::query(format!(
            "INSERT INTO mail.address_index (user_id, email, name, use_count, last_used_at)
               VALUES ($1, $2, $3, $4, $5){upsert}"
        ))
        .bind(user_id)
        .bind(email.to_lowercase())
        .bind(name)
        .bind(weight)
        .bind(chrono::Utc::now())
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
