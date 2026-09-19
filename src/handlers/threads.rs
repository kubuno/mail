use axum::{
    extract::{Path, Query, State},
    Json,
};
use sqlx::{Postgres, QueryBuilder};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{EmailMessage, SnoozeDto, Thread, ThreadListQuery},
    state::AppState,
};
use chrono::{DateTime, Utc};

/// Row shape for Bayesian spam training over a thread's messages.
type SpamTrainRow = (Uuid, String, Option<String>, String, Option<i16>);
/// Row shape for the subscriptions aggregation query.
type SubscriptionRow = (String, Option<String>, Option<String>, i64, DateTime<Utc>);

/// Stored inbox category of a thread. It is written once, when the message is
/// stored (services::categorize), so reading a tab is a plain indexed equality
/// test — no classification happens at display time any more. COALESCE only
/// covers rows written before the column existed.
const CATEGORY_SQL: &str = "COALESCE(t.category, 'main')";

pub async fn list_threads(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ThreadListQuery>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Account delegation: when `on_behalf_of` names a grantor and an accepted
    // delegation authorises this caller, the whole listing scopes to the
    // grantor's mailbox. Without it, `user` is unchanged (self).
    let acting_id = crate::services::delegation::resolve_acting_user(&state.db, &user, q.on_behalf_of).await?;
    let user = AuthUser { id: acting_id, ..user };

    let limit = q.limit.unwrap_or(50).min(100);
    let folder = q.folder.as_deref().unwrap_or("inbox").to_string();

    let threads: Vec<Thread> = if let Some(raw) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // SEARCH: dynamic query compiled from the Gmail-style operator language
        // (see services::search_query). Spam & trash are excluded by default,
        // like Gmail, unless the query names a location explicitly (`in:`).
        let parsed = crate::services::search_query::parse(raw);
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "SELECT DISTINCT t.id, t.account_id, t.user_id, t.subject, \
                    t.message_count, t.unread_count, t.has_attachments, \
                    t.is_starred, t.is_important, t.snippet, t.last_sender_name, t.last_sender_email, \
                    t.last_message_at, t.created_at \
             FROM mail.threads t JOIN mail.messages m ON m.thread_id = t.id \
             WHERE t.user_id = ",
        );
        qb.push_bind(user.id);
        qb.push(" AND m.is_deleted = FALSE");
        if let Some(acc) = q.account_id {
            qb.push(" AND t.account_id = ").push_bind(acc);
        }
        if !parsed.has_in {
            qb.push(" AND m.folder NOT IN ('spam', 'trash')");
        }
        qb.push(" AND ");
        crate::services::search_query::push_sql(&mut qb, &parsed.root, user.id);
        if let Some(before) = q.before {
            qb.push(" AND t.last_message_at < ").push_bind(before);
        }
        qb.push(" ORDER BY t.last_message_at DESC LIMIT ").push_bind(limit);
        qb.build_query_as::<Thread>().fetch_all(&state.db).await?
    } else if q.important == Some(true) {
        sqlx::query_as::<_, Thread>(
            r#"SELECT t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               WHERE t.user_id = $1
                 AND t.is_important = TRUE
                 AND ($2::uuid IS NULL OR t.account_id = $2)
                 AND ($3::timestamptz IS NULL OR t.last_message_at < $3)
               ORDER BY t.last_message_at DESC
               LIMIT $4"#,
        )
        .bind(user.id)
        .bind(q.account_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else if q.snoozed == Some(true) {
        // En attente : fils dont le réveil est dans le futur.
        sqlx::query_as::<_, Thread>(
            r#"SELECT t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               WHERE t.user_id = $1
                 AND t.snoozed_until > NOW()
                 AND ($2::uuid IS NULL OR t.account_id = $2)
                 AND ($3::timestamptz IS NULL OR t.snoozed_until < $3)
               ORDER BY t.snoozed_until ASC
               LIMIT $4"#,
        )
        .bind(user.id)
        .bind(q.account_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else if let Some(label_id) = q.label_id {
        sqlx::query_as::<_, Thread>(
            r#"SELECT t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               JOIN mail.thread_labels tl ON tl.thread_id = t.id
               WHERE t.user_id = $1
                 AND tl.label_id = $2
                 AND ($3::timestamptz IS NULL OR t.last_message_at < $3)
               ORDER BY t.last_message_at DESC
               LIMIT $4"#,
        )
        .bind(user.id)
        .bind(label_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else if q.starred == Some(true) {
        sqlx::query_as::<_, Thread>(
            r#"SELECT t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               WHERE t.user_id = $1
                 AND t.is_starred = TRUE
                 AND ($2::uuid IS NULL OR t.account_id = $2)
                 AND ($3::timestamptz IS NULL OR t.last_message_at < $3)
               ORDER BY t.last_message_at DESC
               LIMIT $4"#,
        )
        .bind(user.id)
        .bind(q.account_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else {
        // Audited: `CATEGORY_SQL` is a constant; the folder, the account, the
        // cursor, the limit, the category and the IMAP folder are all bound.
        sqlx::query_as::<_, Thread>(sqlx::AssertSqlSafe(format!(
            r#"SELECT DISTINCT ON (t.id, t.last_message_at) t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               JOIN mail.messages m ON m.thread_id = t.id
               WHERE t.user_id = $1
                 AND ($2 = 'all' OR m.folder = $2)
                 AND m.is_deleted = FALSE
                 -- snoozed threads leave the inbox until they wake up
                 AND ($2 <> 'inbox' OR t.snoozed_until IS NULL OR t.snoozed_until <= NOW())
                 -- muted threads never come back to the inbox
                 AND ($2 <> 'inbox' OR NOT t.is_muted)
                 AND ($3::uuid IS NULL OR t.account_id = $3)
                 AND ($4::timestamptz IS NULL OR t.last_message_at < $4)
                 AND ($6::text IS NULL OR ({CATEGORY_SQL}) = $6)
                 AND ($7::text IS NULL OR m.imap_folder = $7)
               ORDER BY t.last_message_at DESC
               LIMIT $5"#,
        )))
        .bind(user.id)
        .bind(&folder)
        .bind(q.account_id)
        .bind(q.before)
        .bind(limit)
        .bind(&q.category)
        .bind(&q.imap_folder)
        .fetch_all(&state.db)
        .await?
    };

    let has_more = threads.len() as i64 == limit;
    let cursor   = threads.last().map(|t| t.last_message_at);

    // Total matching the filter actually used above. Every branch computes its
    // own: the client used to fall back to the sidebar counters, which count
    // UNREAD threads per label — a label view with 3 read threads then showed
    // "1–3 of 0". Only search keeps a client-side total.
    let special_total: Option<i64> = if q.search.is_some() {
        None
    } else if let Some(label_id) = q.label_id {
        sqlx::query_scalar(
            "SELECT COUNT(DISTINCT t.id) FROM mail.threads t \
             JOIN mail.thread_labels tl ON tl.thread_id = t.id \
             WHERE t.user_id = $1 AND tl.label_id = $2",
        )
        .bind(user.id)
        .bind(label_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| tracing::error!(error = %e, "list_threads: total du libellé"))
        .ok()
    } else if q.starred == Some(true) || q.important == Some(true) || q.snoozed == Some(true) {
        let predicate = if q.starred == Some(true) {
            "t.is_starred = TRUE"
        } else if q.important == Some(true) {
            "t.is_important = TRUE"
        } else {
            "t.snoozed_until > NOW()"
        };
        // Audited: `predicate` is one of the three literals just above, chosen by
        // the view asked for — no caller text reaches the SQL.
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM mail.threads t \
             WHERE t.user_id = $1 AND {predicate} \
               AND ($2::uuid IS NULL OR t.account_id = $2)",
        )))
        .bind(user.id)
        .bind(q.account_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| tracing::error!(error = %e, "list_threads: total de la vue"))
        .ok()
    } else {
        None
    };

    let total: Option<i64> = if let Some(t) = special_total {
        Some(t)
    } else if q.search.is_none()
        && q.important != Some(true)
        && q.snoozed != Some(true)
        && q.starred != Some(true)
        && q.label_id.is_none()
    {
        // Audited: `CATEGORY_SQL` is a constant; the folder, the account, the
        // category and the IMAP folder asked for are bound parameters.
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            r#"SELECT COUNT(DISTINCT t.id)
               FROM mail.threads t
               JOIN mail.messages m ON m.thread_id = t.id
               WHERE t.user_id = $1
                 AND ($2 = 'all' OR m.folder = $2)
                 AND m.is_deleted = FALSE
                 AND ($2 <> 'inbox' OR t.snoozed_until IS NULL OR t.snoozed_until <= NOW())
                 AND ($2 <> 'inbox' OR NOT t.is_muted)
                 AND ($3::uuid IS NULL OR t.account_id = $3)
                 AND ($4::text IS NULL OR ({CATEGORY_SQL}) = $4)
                 AND ($5::text IS NULL OR m.imap_folder = $5)"#,
        )))
        .bind(user.id)
        .bind(&folder)
        .bind(q.account_id)
        .bind(&q.category)
        .bind(&q.imap_folder)
        .fetch_one(&state.db)
        .await
        .map_err(|e| tracing::error!(error = %e, "list_threads: total du filtre"))
        .ok()
    } else {
        None
    };

    let threads_json = enrich_threads(&state.db, user.id, &threads).await;

    Ok(Json(serde_json::json!({
        "threads":  threads_json,
        "has_more": has_more,
        "cursor":   cursor,
        "total":    total,
    })))
}

/// Turns a page of `Thread` rows into the enriched JSON the clients expect:
/// each thread carries its user labels, the system folders its messages sit in,
/// the newest message's `List-Unsubscribe`, its category, and attachment chips.
///
/// Factored out so `list_threads` and the delta endpoint (`changes`) return the
/// exact same thread shape from one place. Every lookup is a single set-based
/// query keyed on the page's thread ids; a query failure degrades to empty
/// chips rather than failing the whole listing.
pub(crate) async fn enrich_threads(
    db: &sqlx::PgPool,
    user_id: Uuid,
    threads: &[Thread],
) -> Vec<serde_json::Value> {
    // Row chips: the user labels carried by each thread, plus the system
    // folders its messages sit in. Two set-based queries, instead of joining
    // both into every branch of the selection above.
    let ids: Vec<Uuid> = threads.iter().map(|t| t.id).collect();

    let category_rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, category FROM mail.threads WHERE id = ANY($1) AND user_id = $2",
    )
    .bind(&ids)
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(|e| tracing::error!(error = %e, "enrich_threads: catégories"))
    .unwrap_or_default();
    let category_by_thread: HashMap<Uuid, Option<String>> = category_rows.into_iter().collect();

    let label_rows: Vec<(Uuid, Uuid, String, Option<String>)> = sqlx::query_as(
        r#"SELECT tl.thread_id, l.id, l.name, l.color
           FROM mail.thread_labels tl
           JOIN mail.labels l ON l.id = tl.label_id
           WHERE tl.thread_id = ANY($1)
             AND l.user_id = $2
             AND NOT l.is_system
             AND l.message_list_visibility <> 'hide'"#,
    )
    .bind(&ids)
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(|e| tracing::error!(error = %e, "enrich_threads: libellés des fils"))
    .unwrap_or_default();

    let folder_rows: Vec<(Uuid, String)> = sqlx::query_as(
        r#"SELECT DISTINCT m.thread_id, m.folder
           FROM mail.messages m
           WHERE m.thread_id = ANY($1) AND m.user_id = $2 AND NOT m.is_deleted"#,
    )
    .bind(&ids)
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(|e| tracing::error!(error = %e, "enrich_threads: dossiers des fils"))
    .unwrap_or_default();

    let mut labels_by_thread: HashMap<Uuid, Vec<serde_json::Value>> = HashMap::new();
    for (thread_id, id, name, color) in label_rows {
        labels_by_thread
            .entry(thread_id)
            .or_default()
            .push(serde_json::json!({ "id": id, "name": name, "color": color }));
    }

    // List-Unsubscribe of the newest message, so the list can offer Gmail's
    // "Unsubscribe" affordance on hover without opening the conversation.
    let unsub_rows: Vec<(Uuid, String)> = sqlx::query_as(
        r#"SELECT DISTINCT ON (m.thread_id) m.thread_id, m.list_unsubscribe
           FROM mail.messages m
           WHERE m.thread_id = ANY($1) AND m.user_id = $2
             AND m.list_unsubscribe IS NOT NULL AND m.list_unsubscribe <> ''
           ORDER BY m.thread_id, m.received_at DESC"#,
    )
    .bind(&ids)
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(|e| tracing::error!(error = %e, "enrich_threads: List-Unsubscribe des fils"))
    .unwrap_or_default();
    let unsub_by_thread: HashMap<Uuid, String> = unsub_rows.into_iter().collect();

    // Attachment chips on the row: name + mime of the newest message's files.
    let att_rows: Vec<(Uuid, Uuid, serde_json::Value)> = sqlx::query_as(
        r#"SELECT DISTINCT ON (m.thread_id) m.thread_id, m.id, m.attachments
           FROM mail.messages m
           WHERE m.thread_id = ANY($1) AND m.user_id = $2
             AND m.attachments IS NOT NULL AND jsonb_array_length(m.attachments) > 0
           ORDER BY m.thread_id, m.received_at DESC"#,
    )
    .bind(&ids)
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(|e| tracing::error!(error = %e, "enrich_threads: pièces jointes"))
    .unwrap_or_default();

    // Each chip needs the message it belongs to and its index, so the frontend
    // can build the download URL without opening the conversation. The server
    // `storage_path` is deliberately NOT surfaced here.
    let mut atts_by_thread: HashMap<Uuid, serde_json::Value> = HashMap::new();
    for (thread_id, message_id, atts) in att_rows {
        let list: Vec<serde_json::Value> = atts
            .as_array()
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, att)| {
                        serde_json::json!({
                            "name":       att.get("name"),
                            "mime":       att.get("mime"),
                            "size":       att.get("size"),
                            "message_id": message_id,
                            "index":      i,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        atts_by_thread.insert(thread_id, serde_json::json!(list));
    }

    let mut folders_by_thread: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (thread_id, folder) in folder_rows {
        folders_by_thread.entry(thread_id).or_default().push(folder);
    }

    threads
        .iter()
        .map(|t| {
            let mut value = serde_json::to_value(t).unwrap_or_else(|e| {
                tracing::error!(error = %e, thread_id = %t.id, "enrich_threads: sérialisation");
                serde_json::json!({})
            });
            if let Some(obj) = value.as_object_mut() {
                obj.insert(
                    "labels".into(),
                    serde_json::json!(labels_by_thread.get(&t.id).cloned().unwrap_or_default()),
                );
                obj.insert(
                    "folders".into(),
                    serde_json::json!(folders_by_thread.get(&t.id).cloned().unwrap_or_default()),
                );
                obj.insert(
                    "list_unsubscribe".into(),
                    serde_json::json!(unsub_by_thread.get(&t.id)),
                );
                obj.insert(
                    "category".into(),
                    serde_json::json!(category_by_thread.get(&t.id).cloned().flatten()),
                );
                obj.insert(
                    "attachments".into(),
                    atts_by_thread.get(&t.id).cloned().unwrap_or_else(|| serde_json::json!([])),
                );
            }
            value
        })
        .collect()
}

/// Query for the delta-sync endpoint.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChangesQuery {
    /// CONDSTORE cursor: return threads with a message `modseq` strictly greater
    /// than this. `0` or absent = a full sync.
    pub since:      Option<i64>,
    /// Page size, default 200, capped at 500.
    pub limit:      Option<i64>,
    /// Restrict to one account; absent aggregates every account of the user.
    pub account_id: Option<Uuid>,
    /// Act on another user's mailbox (account delegation); see `resolve_acting_user`.
    pub on_behalf_of: Option<Uuid>,
}

/// Delta synchronisation for offline-first clients (the mobile apps): returns
/// only what changed since the client's last cursor, instead of repaginating
/// `GET /threads`.
///
/// Built on the CONDSTORE `modseq` already stamped on every message write
/// (migration 000021): a single keyset page over every thread with a message
/// `modseq > since`. Thread-level flag actions bump their messages' modseq (see
/// [`bump_thread_modseq`]) so they surface here too. Scoped to the caller by
/// `X-Kubuno-User-Id`, exactly like every other route.
pub async fn changes(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ChangesQuery>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Delegation: scope the delta to the grantor's mailbox when authorised.
    let acting_id = crate::services::delegation::resolve_acting_user(&state.db, &user, q.on_behalf_of).await?;
    let user = AuthUser { id: acting_id, ..user };

    let since = q.since.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(200).clamp(1, 500);

    // One keyset page over every thread that changed since `since`: for each,
    // its current HIGHESTMODSEQ (max over ALL its messages) and whether it is
    // now entirely deleted/trashed. Ordered by that modseq ascending so the
    // client resumes from `cursor` with neither a gap nor a duplicate — modseq
    // values are globally unique, so a thread's max is unique too.
    let rows: Vec<(Uuid, i64, bool)> = sqlx::query_as(
        r#"WITH changed AS (
               SELECT DISTINCT thread_id
               FROM mail.messages
               WHERE user_id = $1 AND modseq > $2
                 AND ($3::uuid IS NULL OR account_id = $3)
           )
           SELECT m.thread_id,
                  MAX(m.modseq)                                AS max_modseq,
                  bool_and(m.is_deleted OR m.folder = 'trash') AS fully_deleted
           FROM mail.messages m
           JOIN changed c ON c.thread_id = m.thread_id
           WHERE m.user_id = $1
           GROUP BY m.thread_id
           ORDER BY max_modseq ASC
           LIMIT $4"#,
    )
    .bind(user.id)
    .bind(since)
    .bind(q.account_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let has_more = rows.len() as i64 == limit;
    // Largest modseq covered by this page (live threads AND deletions), or
    // `since` unchanged on an empty page.
    let cursor = rows.iter().map(|(_, mx, _)| *mx).max().unwrap_or(since);

    let mut deleted_ids: Vec<Uuid> = Vec::new();
    let mut live_ids:    Vec<Uuid> = Vec::new();
    for (id, _, fully_deleted) in &rows {
        if *fully_deleted {
            deleted_ids.push(*id);
        } else {
            live_ids.push(*id);
        }
    }

    // Fetch the live threads, then re-order them to the page order (ascending
    // modseq) so the enriched output keeps the keyset order.
    let mut threads_json: Vec<serde_json::Value> = Vec::new();
    if !live_ids.is_empty() {
        let fetched: Vec<Thread> = sqlx::query_as::<_, Thread>(
            r#"SELECT id, account_id, user_id, subject,
                      message_count, unread_count, has_attachments,
                      is_starred, is_important, snippet,
                      last_sender_name, last_sender_email,
                      last_message_at, created_at
               FROM mail.threads WHERE id = ANY($1) AND user_id = $2"#,
        )
        .bind(&live_ids)
        .bind(user.id)
        .fetch_all(&state.db)
        .await?;

        let by_id: HashMap<Uuid, Thread> = fetched.into_iter().map(|t| (t.id, t)).collect();
        let ordered: Vec<Thread> = live_ids.iter().filter_map(|id| by_id.get(id).cloned()).collect();
        threads_json = enrich_threads(&state.db, user.id, &ordered).await;
    }

    Ok(Json(serde_json::json!({
        "threads":     threads_json,
        "deleted_ids": deleted_ids,
        "cursor":      cursor.to_string(),
        "has_more":    has_more,
    })))
}

/// Advance the CONDSTORE `modseq` of every message in a thread.
///
/// Thread-level flag actions (star, important, mute, snooze, category, labels)
/// write only to `mail.threads`/`mail.thread_labels`, which the delta endpoint
/// — keyed on `mail.messages.modseq` — would otherwise miss. A no-op UPDATE on
/// the thread's messages fires the `BEFORE UPDATE` trigger that stamps a fresh
/// modseq (migration 000021), so the change surfaces in the next
/// `GET /changes`. Best-effort: the user-visible action has already committed;
/// a failed bump only means a delta client falls back to a fuller sync, so it
/// is logged rather than fatal.
pub(crate) async fn bump_thread_modseq(db: &sqlx::PgPool, user_id: Uuid, thread_id: Uuid) {
    if let Err(e) = sqlx::query(
        "UPDATE mail.messages SET modseq = modseq WHERE thread_id = $1 AND user_id = $2",
    )
    .bind(thread_id)
    .bind(user_id)
    .execute(db)
    .await
    {
        tracing::warn!(error = %e, %thread_id, "bump modseq du fil échoué");
    }
}

pub async fn get_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<crate::models::OnBehalfQuery>,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Delegation: read the grantor's thread when authorised.
    let acting_id = crate::services::delegation::resolve_acting_user(&state.db, &user, q.on_behalf_of).await?;
    let user = AuthUser { id: acting_id, ..user };

    let thread = sqlx::query_as::<_, Thread>(
        r#"SELECT id, account_id, user_id, subject,
                  message_count, unread_count, has_attachments,
                  is_starred, is_important, snippet,
                  last_sender_name, last_sender_email,
                  last_message_at, created_at
           FROM mail.threads WHERE id = $1 AND user_id = $2"#,
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Thread {thread_id}")))?;

    let messages = sqlx::query_as::<_, EmailMessage>(
        r#"SELECT id, thread_id, account_id, user_id, message_id, in_reply_to,
                  imap_uid, imap_folder, from_name, from_email,
                  to_addresses, cc_addresses, bcc_addresses, reply_to,
                  subject, body_text, body_html, attachments,
                  is_read, is_starred, is_deleted, folder, label_ids,
                  sent_at, received_at, created_at, spam_score, list_unsubscribe,
                  mailed_by, signed_by, security, auth_dmarc, structured_data, invite_response
           FROM mail.messages
           WHERE thread_id = $1 AND is_deleted = FALSE
           ORDER BY received_at ASC"#,
    )
    .bind(thread_id)
    .fetch_all(&state.db)
    .await?;
    let mut messages = messages;

    // OpenPGP: the cards are populated from here, so decrypt / verify any PGP/MIME
    // message in place before returning it (a no-op for ordinary mail).
    for m in &mut messages {
        crate::handlers::messages::decode_pgp_in_place(&state, user.id, m).await;
        // Never surface the server-side storage path to the client.
        crate::handlers::messages::strip_storage_paths(m);
    }

    // Reading a thread does NOT mark it read: this GET is prefetched for every
    // visible row, so the side effect marked whole pages of mail as read
    // without anyone opening them — and silently undid "mark as unread" on the
    // next prefetch. The reader now says so explicitly via POST /threads/:id/read.
    Ok(Json(serde_json::json!({ "thread": thread, "messages": messages })))
}

pub async fn star_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let row = sqlx::query_scalar::<_, bool>(
        "UPDATE mail.threads SET is_starred = NOT is_starred WHERE id = $1 AND user_id = $2 RETURNING is_starred"
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Thread {thread_id}")))?;

    // Surface this flag change to delta-sync clients (see bump_thread_modseq).
    bump_thread_modseq(&state.db, user.id, thread_id).await;
    Ok(Json(serde_json::json!({ "is_starred": row })))
}

pub async fn set_thread_category(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    // `null` releases the pin: the thread goes back to the category derived
    // from its newest message when that message was stored.
    let category = body.get("category").and_then(|v| v.as_str()).map(str::to_string);
    if let Some(c) = &category {
        if !crate::services::categorize::is_valid(c) {
            return Err(MailError::Validation(format!("Catégorie invalide: {c}")));
        }
    }

    let updated = sqlx::query(
        "UPDATE mail.threads t SET
           category_pinned = $1 IS NOT NULL,
           category = COALESCE(
             $1,
             (SELECT m.category FROM mail.messages m
              WHERE m.thread_id = t.id AND m.is_deleted = FALSE
              ORDER BY m.sent_at DESC NULLS LAST, m.received_at DESC
              LIMIT 1),
             'main')
         WHERE t.id = $2 AND t.user_id = $3",
    )
    .bind(&category)
    .bind(thread_id)
    .bind(user.id)
    .execute(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, %thread_id, "set_thread_category");
        MailError::Database(e)
    })?;

    if updated.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Thread {thread_id}")));
    }
    bump_thread_modseq(&state.db, user.id, thread_id).await;
    Ok(Json(serde_json::json!({ "category": category })))
}

pub async fn move_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    let folder = body["folder"]
        .as_str()
        .ok_or_else(|| MailError::Validation("folder requis".into()))?
        .to_string();

    // « archive » = hors boîte de réception mais conservé (visible dans « Tous les messages »).
    if !["inbox", "sent", "spam", "trash", "archive"].contains(&folder.as_str()) {
        return Err(MailError::Validation(format!("Dossier invalide: {folder}")));
    }

    sqlx::query("UPDATE mail.messages SET folder = $1 WHERE thread_id = $2 AND user_id = $3")
        .bind(&folder)
        .bind(thread_id)
        .bind(user.id)
        .execute(&state.db)
        .await?;

    // Feedback bayésien : marquer comme spam (folder='spam') ou « pas spam »
    // (folder='inbox') entraîne le classifieur sur les messages du fil.
    if folder == "spam" || folder == "inbox" {
        let is_spam = folder == "spam";
        let msgs: Vec<SpamTrainRow> = sqlx::query_as(
            "SELECT id, subject, body_text, from_email, spam_trained
             FROM mail.messages WHERE thread_id = $1 AND user_id = $2",
        )
        .bind(thread_id)
        .bind(user.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for (id, subject, body, from_email, prev) in msgs {
            match crate::services::spam_classifier::learn_message(
                &state.db, user.id, &subject, body.as_deref(), &from_email, is_spam, prev,
            ).await {
                Ok(guard) => {
                    let _ = sqlx::query("UPDATE mail.messages SET spam_trained = $1, spam_score = NULL WHERE id = $2")
                        .bind(guard).bind(id).execute(&state.db).await;
                }
                Err(e) => tracing::warn!(error = %e, "Entraînement spam (feedback) échoué"),
            }
        }
    }

    Ok(Json(serde_json::json!({ "message": "Thread déplacé" })))
}

pub async fn delete_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let exists: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM mail.threads WHERE id = $1 AND user_id = $2)"
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    if !exists {
        return Err(MailError::NotFound(format!("Thread {thread_id}")));
    }

    sqlx::query(
        "UPDATE mail.messages SET is_deleted = TRUE, folder = 'trash' WHERE thread_id = $1 AND user_id = $2"
    )
    .bind(thread_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "message": "Thread supprimé" })))
}

// ── Important / En attente (snooze) / Abonnements ────────────────────────────
pub async fn important_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let row = sqlx::query_scalar::<_, bool>(
        "UPDATE mail.threads SET is_important = NOT is_important WHERE id = $1 AND user_id = $2 RETURNING is_important",
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Thread {thread_id}")))?;
    bump_thread_modseq(&state.db, user.id, thread_id).await;
    Ok(Json(serde_json::json!({ "is_important": row })))
}

pub async fn mute_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let row = sqlx::query_scalar::<_, bool>(
        "UPDATE mail.threads SET is_muted = NOT is_muted WHERE id = $1 AND user_id = $2 RETURNING is_muted",
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Thread {thread_id}")))?;
    bump_thread_modseq(&state.db, user.id, thread_id).await;
    Ok(Json(serde_json::json!({ "is_muted": row })))
}

pub async fn snooze_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
    Json(dto): Json<SnoozeDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let res = sqlx::query(
        "UPDATE mail.threads SET snoozed_until = $1 WHERE id = $2 AND user_id = $3",
    )
    .bind(dto.until)
    .bind(thread_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Thread {thread_id}")));
    }
    bump_thread_modseq(&state.db, user.id, thread_id).await;
    Ok(Json(serde_json::json!({ "snoozed_until": dto.until })))
}

// Marquer TOUT un fil comme lu / non lu (action groupée façon Gmail).
pub async fn read_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    let is_read = body["is_read"].as_bool().unwrap_or(true);
    sqlx::query("UPDATE mail.messages SET is_read = $1 WHERE thread_id = $2 AND user_id = $3")
        .bind(is_read).bind(thread_id).bind(user.id)
        .execute(&state.db).await?;
    let unread: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.messages WHERE thread_id = $1 AND is_read = FALSE AND is_deleted = FALSE",
    )
    .bind(thread_id).fetch_one(&state.db).await.unwrap_or(0);
    sqlx::query("UPDATE mail.threads SET unread_count = $1 WHERE id = $2")
        .bind(unread as i32).bind(thread_id).execute(&state.db).await?;
    Ok(Json(serde_json::json!({ "unread_count": unread })))
}

pub async fn subscriptions(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let rows: Vec<SubscriptionRow> = sqlx::query_as(
        r#"SELECT m.from_email,
                  MAX(m.from_name)                                                      AS from_name,
                  (ARRAY_AGG(m.list_unsubscribe ORDER BY m.received_at DESC))[1]         AS list_unsubscribe,
                  COUNT(*)                                                              AS cnt,
                  MAX(m.received_at)                                                    AS last_at
           FROM mail.messages m
           JOIN mail.threads t ON t.id = m.thread_id
           WHERE t.user_id = $1 AND m.list_unsubscribe IS NOT NULL AND m.is_deleted = FALSE
           GROUP BY m.from_email
           ORDER BY cnt DESC
           LIMIT 500"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let subs: Vec<serde_json::Value> = rows.into_iter().map(|(email, name, unsub, cnt, last)| {
        serde_json::json!({
            "from_email":       email,
            "from_name":        name,
            "list_unsubscribe": unsub,
            "count":            cnt,
            "last_at":          last,
        })
    }).collect();
    Ok(Json(serde_json::json!({ "subscriptions": subs })))
}

// ── Compteurs pour la barre latérale (non-lus par dossier, brouillons, libellés) ──
pub async fn counts(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<crate::models::OnBehalfQuery>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Delegation: count the grantor's mailbox when authorised.
    let acting_id = crate::services::delegation::resolve_acting_user(&state.db, &user, q.on_behalf_of).await?;
    let user = AuthUser { id: acting_id, ..user };

    // Non-lus par dossier
    let unread_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"SELECT m.folder, COUNT(*)
           FROM mail.messages m
           JOIN mail.threads t ON t.id = m.thread_id
           WHERE t.user_id = $1 AND m.is_read = FALSE AND m.is_deleted = FALSE
           GROUP BY m.folder"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let mut unread = serde_json::Map::new();
    for (folder, n) in unread_rows {
        unread.insert(folder, serde_json::json!(n));
    }

    // Total fils par dossier (pour les badges « tous »)
    let total_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"SELECT m.folder, COUNT(DISTINCT t.id)
           FROM mail.messages m
           JOIN mail.threads t ON t.id = m.thread_id
           WHERE t.user_id = $1 AND m.is_deleted = FALSE
           GROUP BY m.folder"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let mut total = serde_json::Map::new();
    for (folder, n) in total_rows {
        total.insert(folder, serde_json::json!(n));
    }

    let drafts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mail.drafts WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let starred: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.threads WHERE user_id = $1 AND is_starred = TRUE",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let important: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.threads WHERE user_id = $1 AND is_important = TRUE",
    )
    .bind(user.id).fetch_one(&state.db).await.unwrap_or(0);

    let snoozed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.threads WHERE user_id = $1 AND snoozed_until > NOW()",
    )
    .bind(user.id).fetch_one(&state.db).await.unwrap_or(0);

    let scheduled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.drafts WHERE user_id = $1 AND scheduled_at IS NOT NULL",
    )
    .bind(user.id).fetch_one(&state.db).await.unwrap_or(0);

    // Unread threads per label — Gmail's sidebar badge counts unread, not total.
    let label_rows: Vec<(Uuid, i64)> = sqlx::query_as(
        r#"SELECT tl.label_id, COUNT(DISTINCT tl.thread_id)
           FROM mail.thread_labels tl
           JOIN mail.threads t ON t.id = tl.thread_id
           WHERE t.user_id = $1 AND t.unread_count > 0
           GROUP BY tl.label_id"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut labels = serde_json::Map::new();
    for (id, n) in label_rows {
        labels.insert(id.to_string(), serde_json::json!(n));
    }

    Ok(Json(serde_json::json!({
        "unread":    unread,
        "total":     total,
        "drafts":    drafts,
        "starred":   starred,
        "important": important,
        "snoozed":   snoozed,
        "scheduled": scheduled,
        "labels":    labels,
    })))
}
