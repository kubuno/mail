//! `/admin/mailboxes` — the local addresses this instance files into a Kubuno
//! account, and the IMAP/SMTP login that goes with one.
//!
//! ── What the module cannot know about the owner ──────────────────────────────
//! A mailbox names a Kubuno account by its `user_id`, but the module owns no
//! access to `core.users` (schema `mail` only, and the core exposes no internal
//! route that lists or resolves a user — see the module's README of this change
//! and the report of this work). So `user_id` is accepted as given, and what CAN
//! be checked is checked: it is a UUID, and `owner_known` reports whether that
//! id is one the mail schema has ever seen (an account, a credential, or stored
//! messages). A mailbox whose owner is unknown is not refused — the account may
//! simply never have used mail — it is flagged, which is what the migration
//! asked the panel to do rather than silently swallowing mail.
//!
//! ── Why occupancy is a count and not a size ──────────────────────────────────
//! `mail.messages` stores no message size: no raw RFC 5322 bytes, no
//! `rfc822_size` column, only parsed fields (see migration 000003). Any byte
//! figure would therefore be an invention, so none is returned — `used_bytes` is
//! `null` and the list says so once. Worse, a message records `user_id` and
//! `account_id` but never the local address it was delivered TO, so even a size
//! column would not be attributable to one mailbox among several of the same
//! owner. `owner_message_count` is therefore named for what it actually counts.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    db_error, generate_password, local_domains, parse_address, require_address_free, require_admin,
    require_local_domain, translate_conflict, ListQuery,
};
use crate::{
    errors::MailError, middleware::AuthUser, server::auth, services::crypto::MailCrypto,
    state::AppState,
};

/// Said once per list rather than per row: the reason there is no byte figure.
const USAGE_NOTE: &str = "L'occupation en octets n'est pas disponible : mail.messages ne conserve \
     ni les octets bruts du message ni sa taille, et un message n'enregistre pas l'adresse locale \
     à laquelle il a été distribué. Le nombre de messages est celui du COMPTE propriétaire, pas \
     de cette seule adresse.";

const COLUMNS: &str = "id, address, domain, user_id, display_name, quota_bytes, is_active, \
                       comment, created_at, updated_at";

#[derive(Debug, sqlx::FromRow)]
pub struct MailboxRow {
    pub id: Uuid,
    pub address: String,
    pub domain: String,
    pub user_id: Uuid,
    pub display_name: Option<String>,
    pub quota_bytes: i64,
    pub is_active: bool,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct MailboxView {
    pub id: Uuid,
    pub address: String,
    pub domain: String,
    pub user_id: Uuid,
    pub display_name: Option<String>,
    /// 0 = illimité.
    pub quota_bytes: i64,
    pub is_active: bool,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// True when the domain is still one of `server_domains`. A mailbox in a
    /// domain the instance stopped serving receives nothing and must say so.
    /// `null` when the core could not be reached: unknown is not the same
    /// answer as no, and a panel showing "not served" for every row because a
    /// settings read timed out would send an operator chasing a fault that is
    /// not there.
    pub domain_served: Option<bool>,
    /// True when this `user_id` is known to the mail schema. The module cannot
    /// read `core.users`, so this is a hint, not a proof of existence.
    pub owner_known: bool,
    /// True when an IMAP/SMTP login exists for this exact address.
    pub has_credential: bool,
    /// Messages stored for the OWNER account (all its addresses together),
    /// excluding those already flagged deleted.
    pub owner_message_count: i64,
    /// Always `null` — see `USAGE_NOTE`.
    pub used_bytes: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct MailboxPage {
    pub items: Vec<MailboxView>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    /// The `server_domains` setting as read for this request, or `null` when the
    /// core was unreachable.
    pub served_domains: Option<Vec<String>>,
    /// Whether `used_bytes` can ever be filled, and why not.
    pub usage_bytes_available: bool,
    pub usage_note: &'static str,
}

/// A freshly issued IMAP/SMTP login. The password appears here and nowhere else
/// — it is not stored (only its Argon2 hash and SCRAM secret are) and it is
/// never logged.
#[derive(Debug, Serialize)]
pub struct CreatedCredential {
    pub id: Uuid,
    pub username: String,
    pub password: String,
    pub note: &'static str,
}

const CREDENTIAL_NOTE: &str =
    "Ce mot de passe n'est affiché qu'une fois : il n'est pas conservé et ne peut pas être réaffiché.";

#[derive(Debug, Serialize)]
pub struct CreatedMailbox {
    pub mailbox: MailboxView,
    pub credential: Option<CreatedCredential>,
}

#[derive(Debug, Deserialize)]
pub struct CreateMailboxDto {
    pub address: String,
    /// The Kubuno account this address files into.
    pub user_id: Uuid,
    pub display_name: Option<String>,
    /// Omitted means the domain's `default_quota_bytes`; 0 = unlimited.
    pub quota_bytes: Option<i64>,
    pub is_active: Option<bool>,
    pub comment: Option<String>,
    /// Also issue the IMAP/SMTP login for this address, returned once.
    pub create_credential: Option<bool>,
    pub credential_label: Option<String>,
    /// Reset a login that already exists for this address. Off by default: a
    /// credential may predate this table, and silently rotating it would break
    /// a mail client that has been working for months.
    pub replace_credential: Option<bool>,
}

/// PATCH semantics: an omitted field is unchanged. For the two free-text fields
/// an empty string clears the value, since JSON `null` and "absent" are the same
/// thing to `Option`.
#[derive(Debug, Deserialize)]
pub struct UpdateMailboxDto {
    pub address: Option<String>,
    pub user_id: Option<Uuid>,
    pub display_name: Option<String>,
    pub quota_bytes: Option<i64>,
    pub is_active: Option<bool>,
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteMailboxQuery {
    /// Also delete the IMAP/SMTP login bearing this address. Off by default —
    /// see `CreateMailboxDto::replace_credential` for why.
    pub delete_credential: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct IssueCredentialDto {
    pub label: Option<String>,
}

// ── Read ─────────────────────────────────────────────────────────────────────

pub async fn list_mailboxes(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<MailboxPage>, MailError> {
    require_admin(&user)?;

    let (limit, offset) = q.page();
    let domain = q.domain_filter();
    let pattern = q.pattern();

    let filter = r#"
        WHERE ($1::text IS NULL OR domain = $1)
          AND ($2::boolean IS NULL OR is_active = $2)
          AND ($3::text IS NULL
               OR address LIKE $3 ESCAPE '\'
               OR LOWER(COALESCE(display_name, '')) LIKE $3 ESCAPE '\'
               OR LOWER(COALESCE(comment, '')) LIKE $3 ESCAPE '\')
    "#;

    let total: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM mail.mailboxes {filter}"))
        .bind(&domain)
        .bind(q.active)
        .bind(&pattern)
        .fetch_one(&state.db)
        .await
        .map_err(db_error("comptage des boîtes"))?;

    let rows = sqlx::query_as::<_, MailboxRow>(&format!(
        "SELECT {COLUMNS} FROM mail.mailboxes {filter} ORDER BY address LIMIT $4 OFFSET $5"
    ))
    .bind(&domain)
    .bind(q.active)
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(db_error("liste des boîtes"))?;

    // A read must not fail because the core was briefly unreachable; it reports
    // "unknown" instead (see `MailboxView::domain_served`).
    let served = local_domains(&state).await.ok();
    let items = decorate(&state.db, rows, served.as_deref()).await?;

    Ok(Json(MailboxPage {
        items,
        total,
        limit,
        offset,
        served_domains: served,
        usage_bytes_available: false,
        usage_note: USAGE_NOTE,
    }))
}

pub async fn get_mailbox(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MailboxView>, MailError> {
    require_admin(&user)?;
    let row = fetch_one(&state.db, id).await?;
    let served = local_domains(&state).await.ok();
    let mut views = decorate(&state.db, vec![row], served.as_deref()).await?;
    match views.pop() {
        Some(view) => Ok(Json(view)),
        None => Err(MailError::NotFound(format!("Boîte {id}"))),
    }
}

// ── Write ────────────────────────────────────────────────────────────────────

pub async fn create_mailbox(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<CreateMailboxDto>,
) -> Result<Json<CreatedMailbox>, MailError> {
    require_admin(&user)?;

    // Everything that can be refused is refused before any write, so a rejected
    // request leaves nothing behind.
    let parsed = parse_address(&dto.address)?;
    if parsed.is_catch_all {
        return Err(MailError::Validation(
            "Une boîte ne peut pas être un attrape-tout : « @domaine » ne se crée qu'en alias".into(),
        ));
    }

    let served = local_domains(&state).await?;
    require_local_domain(&served, &parsed.domain)?;
    require_address_free(&state.db, &parsed.address, None).await?;

    let quota = match dto.quota_bytes {
        Some(q) if q < 0 => {
            return Err(MailError::Validation("Le quota ne peut pas être négatif".into()))
        }
        Some(q) => q,
        // The domain's default, or unlimited when the domain has no policy.
        None => default_quota(&state.db, &parsed.domain).await?,
    };

    enforce_mailbox_ceiling(&state.db, &parsed.domain).await?;

    let wants_credential = dto.create_credential.unwrap_or(false);
    if wants_credential && !dto.replace_credential.unwrap_or(false) {
        let exists = credential_exists(&state.db, &parsed.address).await?;
        if exists {
            return Err(MailError::Conflict(format!(
                "Un identifiant IMAP/SMTP existe déjà pour « {} ». Le régénérer changerait le mot \
                 de passe déjà configuré dans un client de messagerie : renvoyez \
                 « replace_credential: true » pour l'assumer.",
                parsed.address
            )));
        }
    }

    // The mailbox and the local account that fronts it are created together, in
    // ONE transaction: a hosted mailbox that appeared nowhere in its owner's mail
    // client is exactly the bug this change fixes, so the two must never exist
    // apart. See migration 000025.
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(db_error("ouverture de la transaction de création"))?;

    let row = sqlx::query_as::<_, MailboxRow>(&format!(
        r#"INSERT INTO mail.mailboxes
             (address, domain, user_id, display_name, quota_bytes, is_active, comment)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING {COLUMNS}"#
    ))
    .bind(&parsed.address)
    .bind(&parsed.domain)
    .bind(dto.user_id)
    .bind(clean_text(dto.display_name.as_deref()))
    .bind(quota)
    .bind(dto.is_active.unwrap_or(true))
    .bind(clean_text(dto.comment.as_deref()))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| translate_conflict(e, &parsed.address, &parsed.domain, "création d'une boîte"))?;

    create_local_account(&mut tx, &state, &row).await?;

    tx.commit()
        .await
        .map_err(db_error("validation de la création de la boîte"))?;

    // The credential is a second table with its own upsert, and `upsert_credential`
    // takes a pool rather than a transaction. Rather than leave a mailbox whose
    // requested login was never issued, a failure here undoes the mailbox AND its
    // local account: the administrator asked for one object, not half of one.
    // The account is deleted first — it references the mailbox `ON DELETE SET
    // NULL`, so removing the mailbox first would orphan it rather than remove it.
    let credential = if wants_credential {
        match issue_credential(&state.db, row.user_id, &row.address, dto.credential_label.as_deref())
            .await
        {
            Ok(cred) => Some(cred),
            Err(e) => {
                if let Err(cleanup) = sqlx::query("DELETE FROM mail.accounts WHERE mailbox_id = $1")
                    .bind(row.id)
                    .execute(&state.db)
                    .await
                {
                    tracing::error!(
                        error = %cleanup,
                        mailbox = %row.id,
                        "annulation du compte local après échec de l'identifiant"
                    );
                }
                if let Err(cleanup) = sqlx::query("DELETE FROM mail.mailboxes WHERE id = $1")
                    .bind(row.id)
                    .execute(&state.db)
                    .await
                {
                    tracing::error!(
                        error = %cleanup,
                        mailbox = %row.id,
                        "annulation de la boîte après échec de l'identifiant"
                    );
                }
                return Err(e);
            }
        }
    } else {
        None
    };

    let mut views = decorate(&state.db, vec![row], Some(&served)).await?;
    let mailbox = views
        .pop()
        .ok_or_else(|| MailError::Internal(anyhow::anyhow!("boîte créée introuvable")))?;

    Ok(Json(CreatedMailbox { mailbox, credential }))
}

pub async fn update_mailbox(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(dto): Json<UpdateMailboxDto>,
) -> Result<Json<MailboxView>, MailError> {
    require_admin(&user)?;

    let current = fetch_one(&state.db, id).await?;
    let served = local_domains(&state).await?;

    // Renaming is allowed, and goes through exactly the checks a creation does.
    let (address, domain) = match dto.address.as_deref() {
        Some(raw) => {
            let parsed = parse_address(raw)?;
            if parsed.is_catch_all {
                return Err(MailError::Validation(
                    "Une boîte ne peut pas être un attrape-tout".into(),
                ));
            }
            require_local_domain(&served, &parsed.domain)?;
            require_address_free(&state.db, &parsed.address, Some(id)).await?;
            (parsed.address, parsed.domain)
        }
        None => (current.address.clone(), current.domain.clone()),
    };

    if let Some(q) = dto.quota_bytes {
        if q < 0 {
            return Err(MailError::Validation("Le quota ne peut pas être négatif".into()));
        }
    }

    let row = sqlx::query_as::<_, MailboxRow>(&format!(
        r#"UPDATE mail.mailboxes SET
             address      = $2,
             domain       = $3,
             user_id      = COALESCE($4, user_id),
             display_name = CASE WHEN $5::text IS NULL THEN display_name
                                 WHEN $5 = '' THEN NULL ELSE $5 END,
             quota_bytes  = COALESCE($6, quota_bytes),
             is_active    = COALESCE($7, is_active),
             comment      = CASE WHEN $8::text IS NULL THEN comment
                                 WHEN $8 = '' THEN NULL ELSE $8 END
           WHERE id = $1
           RETURNING {COLUMNS}"#
    ))
    .bind(id)
    .bind(&address)
    .bind(&domain)
    .bind(dto.user_id)
    .bind(dto.display_name.as_deref().map(str::trim))
    .bind(dto.quota_bytes)
    .bind(dto.is_active)
    .bind(dto.comment.as_deref().map(str::trim))
    .fetch_one(&state.db)
    .await
    .map_err(|e| translate_conflict(e, &address, &domain, "mise à jour d'une boîte"))?;

    // Keep the local account that fronts this mailbox in step: its display name,
    // its enabled state and its address identity follow the mailbox. A no-op when
    // no account is linked (an external-only user, or a legacy mailbox created
    // before 000025).
    let account_name = row.display_name.clone().unwrap_or_else(|| row.address.clone());
    if let Err(e) = sqlx::query(
        "UPDATE mail.accounts
         SET name = $2, is_active = $3, email_address = $4, imap_username = $4, smtp_username = $4
         WHERE mailbox_id = $1",
    )
    .bind(row.id)
    .bind(&account_name)
    .bind(row.is_active)
    .bind(&row.address)
    .execute(&state.db)
    .await
    {
        tracing::error!(error = %e, mailbox = %row.id, "propagation de la boîte vers son compte local échouée");
    }

    let mut views = decorate(&state.db, vec![row], Some(&served)).await?;
    views
        .pop()
        .map(Json)
        .ok_or_else(|| MailError::NotFound(format!("Boîte {id}")))
}

/// Deleting a mailbox stops it accepting mail. It does NOT delete what was
/// already delivered: those messages belong to their owner's account, are read
/// through the ordinary mail views, and erasing them from here would destroy
/// years of correspondence as a side effect of an addressing change. The
/// response says so explicitly, in words the panel can show as-is.
pub async fn delete_mailbox(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<DeleteMailboxQuery>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_admin(&user)?;

    let row = fetch_one(&state.db, id).await?;
    let kept = owner_message_count(&state.db, row.user_id).await?;

    // Deactivate the local account BEFORE dropping the mailbox. Deleting the
    // mailbox only sets `accounts.mailbox_id` to NULL (ON DELETE SET NULL) — the
    // account, and every message filed into it, is KEPT — but its owner must stop
    // composing from an address that no longer accepts mail, so it is disabled.
    if let Err(e) = sqlx::query("UPDATE mail.accounts SET is_active = FALSE WHERE mailbox_id = $1")
        .bind(id)
        .execute(&state.db)
        .await
    {
        tracing::error!(error = %e, mailbox = %id, "désactivation du compte local avant suppression échouée");
    }

    let deleted = sqlx::query("DELETE FROM mail.mailboxes WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(db_error("suppression d'une boîte"))?;
    if deleted.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Boîte {id}")));
    }

    let mut credential_deleted = false;
    if q.delete_credential.unwrap_or(false) {
        let removed = sqlx::query("DELETE FROM mail.mailbox_credentials WHERE username = $1")
            .bind(&row.address)
            .execute(&state.db)
            .await
            .map_err(db_error("suppression de l'identifiant de boîte"))?;
        credential_deleted = removed.rows_affected() > 0;
    }

    let message = format!(
        "L'adresse « {} » n'accepte plus de courrier. Les {kept} message(s) déjà distribués au \
         compte propriétaire sont CONSERVÉS : supprimer une boîte ne supprime aucun message. {}",
        row.address,
        if credential_deleted {
            "L'identifiant IMAP/SMTP portant cette adresse a été supprimé."
        } else if q.delete_credential.unwrap_or(false) {
            "Aucun identifiant IMAP/SMTP ne portait cette adresse."
        } else {
            "L'identifiant IMAP/SMTP portant cette adresse, s'il existe, a été CONSERVÉ \
             (ajoutez ?delete_credential=true pour le supprimer aussi)."
        }
    );

    Ok(Json(serde_json::json!({
        "deleted": true,
        "address": row.address,
        "messages_kept": kept,
        "messages_deleted": 0,
        "credential_deleted": credential_deleted,
        "message": message,
    })))
}

/// Issues (or rotates) the IMAP/SMTP login of an existing mailbox. The password
/// is returned once and never again.
pub async fn issue_mailbox_credential(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(dto): Json<IssueCredentialDto>,
) -> Result<Json<CreatedCredential>, MailError> {
    require_admin(&user)?;
    let row = fetch_one(&state.db, id).await?;
    let cred = issue_credential(&state.db, row.user_id, &row.address, dto.label.as_deref()).await?;
    Ok(Json(cred))
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Creates the `mail.accounts` row (`kind = 'local'`) that fronts a freshly
/// created mailbox, inside the caller's transaction.
///
/// A local account is an ordinary account of its owner: it shows in the client's
/// account list and its "From" selector, its mail is filed into it, and no
/// IMAP/SMTP configuration is needed because the instance itself is the server.
/// The eight NOT NULL external-transport columns are never read for it (the sync
/// worker skips it, sending goes through the instance's outbound queue), so they
/// carry harmless sentinels — empty host, `security = 'none'`, an encrypted empty
/// password. It is told apart from an external account by `kind`. See migration
/// 000025.
/// Attributes an automatic address to one account, resolving a name clash.
///
/// The counterpart of [`create_mailbox`] for the provisioning worker: no
/// `AuthUser` (it is internal), no credential, and it NEVER overwrites — an
/// account that already holds any mailbox on this domain is left exactly as it
/// is, so a manually chosen address, or a previous run, is never disturbed. That
/// is what makes the reconcile safe to run on every tick.
///
/// `local_base` is the local part the rule produced, already sanitised. If it is
/// taken, `local_base2`, `local_base3`… are tried in turn: two people whose rule
/// collapses to the same string still each get an address, and the second one
/// carries the suffix, not the first.
///
/// Returns the address created, or `None` when the account already had one.
pub(crate) async fn provision_mailbox(
    state: &AppState,
    domain: &str,
    user_id: Uuid,
    local_base: &str,
    display_name: Option<&str>,
) -> Result<Option<String>, MailError> {
    // Already served on this domain? Then there is nothing to do — and nothing
    // to overwrite. This is the idempotency the worker relies on.
    let has: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mail.mailboxes WHERE user_id = $1 AND domain = $2 LIMIT 1",
    )
    .bind(user_id)
    .bind(domain)
    .fetch_optional(&state.db)
    .await
    .map_err(db_error("provisioning : boîte existante ?"))?;
    if has.is_some() {
        return Ok(None);
    }

    // Find a free address: base, then base2, base3… A ceiling stops a pathologic
    // domain (everything collapsing to one string) from looping forever; past it
    // the account is left unprovisioned and the worker logs it rather than spin.
    let mut address = String::new();
    let mut found = false;
    for n in 1..=50u32 {
        let candidate = if n == 1 {
            format!("{local_base}@{domain}")
        } else {
            format!("{local_base}{n}@{domain}")
        };
        if require_address_free(&state.db, &candidate, None).await.is_ok() {
            address = candidate;
            found = true;
            break;
        }
    }
    if !found {
        tracing::warn!(user_id = %user_id, domain, local_base, "provisioning : aucune adresse libre après 50 essais");
        return Ok(None);
    }

    let quota = default_quota(&state.db, domain).await?;

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(db_error("provisioning : ouverture de transaction"))?;

    let row = sqlx::query_as::<_, MailboxRow>(&format!(
        r#"INSERT INTO mail.mailboxes
             (address, domain, user_id, display_name, quota_bytes, is_active, comment)
           VALUES ($1, $2, $3, $4, $5, TRUE, $6)
           RETURNING {COLUMNS}"#
    ))
    .bind(&address)
    .bind(domain)
    .bind(user_id)
    .bind(clean_text(display_name))
    .bind(quota)
    .bind(Some("Adresse attribuée automatiquement"))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| translate_conflict(e, &address, domain, "provisioning : création de la boîte"))?;

    create_local_account(&mut tx, state, &row).await?;

    tx.commit()
        .await
        .map_err(db_error("provisioning : validation"))?;

    Ok(Some(address))
}

pub(crate) async fn create_local_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &AppState,
    mailbox: &MailboxRow,
) -> Result<(), MailError> {
    let crypto =
        MailCrypto::new(&state.settings.mail.encryption_key).map_err(|_| MailError::Crypto)?;
    // A sentinel that is never used, but the columns are NOT NULL: an encrypted
    // empty string, so no plaintext (not even "") is stored in the clear.
    let (imap_enc, imap_nonce) = crypto.encrypt("").map_err(|_| MailError::Crypto)?;
    let (smtp_enc, smtp_nonce) = crypto.encrypt("").map_err(|_| MailError::Crypto)?;

    // The owner's FIRST account becomes the default, so a brand-new user with a
    // single hosted mailbox has a working composer with nothing to configure.
    let has_account: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mail.accounts WHERE user_id = $1)")
            .bind(mailbox.user_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(db_error("recherche d'un compte existant"))?;
    let is_default = !has_account;

    // Display name for the "From" identity: the mailbox's own display name when
    // set, else the address's local part (e.g. "test001") rather than the whole
    // address — a name equal to the address would render as `x@y <x@y>`.
    let name = mailbox.display_name.clone().unwrap_or_else(|| {
        mailbox
            .address
            .split('@')
            .next()
            .filter(|p| !p.is_empty())
            .unwrap_or(&mailbox.address)
            .to_string()
    });
    let account_id = Uuid::new_v4();

    // $4 is the address, reused as email_address AND as the imap/smtp usernames.
    sqlx::query(
        r#"INSERT INTO mail.accounts
             (id, user_id, name, email_address, kind, mailbox_id, incoming_protocol,
              imap_host, imap_port, imap_security, imap_username, imap_password, imap_password_nonce,
              smtp_host, smtp_port, smtp_security, smtp_username, smtp_password, smtp_password_nonce,
              auth_kind, is_default, is_active)
           VALUES ($1, $2, $3, $4, 'local', $5, 'imap',
                   '', 0, 'none', $4, $6, $7,
                   '', 0, 'none', $4, $8, $9,
                   'password', $10, $11)"#,
    )
    .bind(account_id)
    .bind(mailbox.user_id)
    .bind(&name)
    .bind(&mailbox.address)
    .bind(mailbox.id)
    .bind(imap_enc.as_slice())
    .bind(imap_nonce.as_slice())
    .bind(smtp_enc.as_slice())
    .bind(smtp_nonce.as_slice())
    .bind(is_default)
    .bind(mailbox.is_active)
    .execute(&mut **tx)
    .await
    .map_err(db_error("création du compte local"))?;

    // The five well-known folders, exactly as an external account gets them, so
    // the sidebar and the folder views have their Inbox/Sent/Drafts/Spam/Trash.
    for (label, folder) in &[
        ("Boîte de réception", "INBOX"),
        ("Envoyés", "Sent"),
        ("Brouillons", "Drafts"),
        ("Spam", "Junk"),
        ("Corbeille", "Trash"),
    ] {
        sqlx::query(
            "INSERT INTO mail.labels (account_id, user_id, name, imap_folder, is_system) \
             VALUES ($1, $2, $3, $4, TRUE)",
        )
        .bind(account_id)
        .bind(mailbox.user_id)
        .bind(label)
        .bind(folder)
        .execute(&mut **tx)
        .await
        .map_err(db_error("création des dossiers système du compte local"))?;
    }

    Ok(())
}

/// Backfills the local `mail.accounts` row for every active mailbox that has
/// none. Run once at startup, after migrations.
///
/// The local-account feature (migration 000025) makes `create_mailbox` create
/// the fronting account, but mailboxes created BEFORE it never got theirs: they
/// receive mail yet appear in no account list and in no "From" selector, so
/// their owner can neither see nor reply from them. This closes that gap.
///
/// Idempotent: it only touches active mailboxes with no linked account, so
/// re-running it on every boot is a no-op once the backlog is cleared (the
/// partial unique index on `accounts.mailbox_id` is the ultimate guarantee). A
/// failure on one mailbox is logged and skipped — it must not strand the others
/// nor block the process from serving.
pub async fn ensure_local_accounts(state: &AppState) -> Result<(), MailError> {
    // NOT EXISTS keeps `mail.mailboxes` the only table in the FROM, so the
    // unqualified `COLUMNS` list stays unambiguous.
    let rows = sqlx::query_as::<_, MailboxRow>(&format!(
        "SELECT {COLUMNS} FROM mail.mailboxes m \
         WHERE m.is_active \
           AND NOT EXISTS (SELECT 1 FROM mail.accounts a WHERE a.mailbox_id = m.id)"
    ))
    .fetch_all(&state.db)
    .await
    .map_err(db_error("boîtes actives sans compte local"))?;

    if rows.is_empty() {
        return Ok(());
    }

    let mut created = 0usize;
    for mailbox in &rows {
        // One transaction per mailbox: the account and its five system folders
        // must appear together, and a failure on one is isolated from the rest.
        let mut tx = match state.db.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!(error = %e, mailbox = %mailbox.id, "ouverture de la transaction de backfill du compte local");
                continue;
            }
        };
        match create_local_account(&mut tx, state, mailbox).await {
            Ok(()) => match tx.commit().await {
                Ok(()) => created += 1,
                Err(e) => tracing::error!(error = %e, mailbox = %mailbox.id, "validation du backfill du compte local"),
            },
            Err(e) => {
                tracing::error!(error = %e, mailbox = %mailbox.id, "backfill du compte local");
                if let Err(rb) = tx.rollback().await {
                    tracing::error!(error = %rb, mailbox = %mailbox.id, "annulation de la transaction de backfill");
                }
            }
        }
    }

    if created > 0 {
        tracing::info!(created, "comptes locaux créés par backfill (boîtes préexistantes à 000025)");
    }
    Ok(())
}

async fn fetch_one(db: &PgPool, id: Uuid) -> Result<MailboxRow, MailError> {
    sqlx::query_as::<_, MailboxRow>(&format!("SELECT {COLUMNS} FROM mail.mailboxes WHERE id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await
        .map_err(db_error("lecture d'une boîte"))?
        .ok_or_else(|| MailError::NotFound(format!("Boîte {id}")))
}

/// Trims a free-text field and turns an empty one into `NULL`.
fn clean_text(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Fills the computed half of the view for a whole page at once: three
/// aggregate queries rather than three per row.
async fn decorate(
    db: &PgPool,
    rows: Vec<MailboxRow>,
    served: Option<&[String]>,
) -> Result<Vec<MailboxView>, MailError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let user_ids: Vec<Uuid> = {
        let mut ids: Vec<Uuid> = rows.iter().map(|r| r.user_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let addresses: Vec<String> = rows.iter().map(|r| r.address.clone()).collect();

    let counts: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT user_id, COUNT(*) FROM mail.messages \
         WHERE user_id = ANY($1) AND is_deleted = FALSE GROUP BY user_id",
    )
    .bind(&user_ids)
    .fetch_all(db)
    .await
    .map_err(db_error("comptage des messages par propriétaire"))?;

    // The only proof of existence available without `core.users`: an id the mail
    // schema has already recorded somewhere.
    let known: Vec<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM mail.accounts WHERE user_id = ANY($1) \
         UNION \
         SELECT user_id FROM mail.mailbox_credentials WHERE user_id = ANY($1)",
    )
    .bind(&user_ids)
    .fetch_all(db)
    .await
    .map_err(db_error("reconnaissance des propriétaires"))?;

    let with_credential: Vec<String> =
        sqlx::query_scalar("SELECT username FROM mail.mailbox_credentials WHERE username = ANY($1)")
            .bind(&addresses)
            .fetch_all(db)
            .await
            .map_err(db_error("identifiants existants"))?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let owner_message_count = counts
                .iter()
                .find(|(id, _)| *id == r.user_id)
                .map(|(_, n)| *n)
                .unwrap_or(0);
            MailboxView {
                domain_served: served.map(|list| list.iter().any(|d| d == &r.domain)),
                owner_known: known.contains(&r.user_id) || owner_message_count > 0,
                has_credential: with_credential.contains(&r.address),
                owner_message_count,
                used_bytes: None,
                id: r.id,
                address: r.address,
                domain: r.domain,
                user_id: r.user_id,
                display_name: r.display_name,
                quota_bytes: r.quota_bytes,
                is_active: r.is_active,
                comment: r.comment,
                created_at: r.created_at,
                updated_at: r.updated_at,
            }
        })
        .collect())
}

async fn owner_message_count(db: &PgPool, user_id: Uuid) -> Result<i64, MailError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM mail.messages WHERE user_id = $1 AND is_deleted = FALSE")
        .bind(user_id)
        .fetch_one(db)
        .await
        .map_err(db_error("comptage des messages du propriétaire"))
}

async fn default_quota(db: &PgPool, domain: &str) -> Result<i64, MailError> {
    let quota: Option<i64> =
        sqlx::query_scalar("SELECT default_quota_bytes FROM mail.domain_policies WHERE domain = $1")
            .bind(domain)
            .fetch_optional(db)
            .await
            .map_err(db_error("quota par défaut du domaine"))?;
    Ok(quota.unwrap_or(0))
}

/// Refuses a creation that would exceed the domain's mailbox ceiling. Never
/// applied retroactively: lowering the ceiling does not delete anything.
async fn enforce_mailbox_ceiling(db: &PgPool, domain: &str) -> Result<(), MailError> {
    let max: Option<i32> =
        sqlx::query_scalar("SELECT max_mailboxes FROM mail.domain_policies WHERE domain = $1")
            .bind(domain)
            .fetch_optional(db)
            .await
            .map_err(db_error("plafond de boîtes du domaine"))?;

    let Some(max) = max.filter(|m| *m > 0) else {
        return Ok(());
    };

    let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mail.mailboxes WHERE domain = $1")
        .bind(domain)
        .fetch_one(db)
        .await
        .map_err(db_error("comptage des boîtes du domaine"))?;

    if used >= i64::from(max) {
        return Err(MailError::Conflict(format!(
            "Le domaine « {domain} » a atteint son plafond de {max} boîte(s)"
        )));
    }
    Ok(())
}

async fn credential_exists(db: &PgPool, address: &str) -> Result<bool, MailError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mail.mailbox_credentials WHERE username = $1)")
        .bind(address)
        .fetch_one(db)
        .await
        .map_err(db_error("existence d'un identifiant de boîte"))
}

/// Generates the password, derives the Argon2 hash and the SCRAM secret through
/// the shared `server::auth` path, and hands the plaintext back exactly once.
/// The plaintext is never logged: no `tracing` call below sees it, and the error
/// path reports only the failure.
async fn issue_credential(
    db: &PgPool,
    user_id: Uuid,
    address: &str,
    label: Option<&str>,
) -> Result<CreatedCredential, MailError> {
    let password = generate_password();
    let id = auth::upsert_credential(db, user_id, address, &password, label)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, address, "création de l'identifiant IMAP/SMTP");
            MailError::Validation(e.to_string())
        })?;

    Ok(CreatedCredential {
        id,
        username: address.to_string(),
        password,
        note: CREDENTIAL_NOTE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_text_is_trimmed_and_emptied_to_null() {
        assert_eq!(clean_text(Some("  Alice  ")).as_deref(), Some("Alice"));
        assert_eq!(clean_text(Some("   ")), None);
        assert_eq!(clean_text(None), None);
    }

    #[test]
    fn a_created_credential_never_carries_a_hash_or_a_secret() {
        // Serialising the view is the only way the password leaves the process;
        // this pins the shape so a later field cannot smuggle a stored secret.
        let cred = CreatedCredential {
            id: Uuid::nil(),
            username: "alice@example.com".into(),
            password: "s3cret".into(),
            note: CREDENTIAL_NOTE,
        };
        let json = serde_json::to_value(&cred).expect("sérialisable");
        let mut keys: Vec<&str> = json
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        assert_eq!(keys, vec!["id", "note", "password", "username"]);
    }

    #[test]
    fn a_mailbox_view_exposes_no_secret() {
        let view = MailboxView {
            id: Uuid::nil(),
            address: "alice@example.com".into(),
            domain: "example.com".into(),
            user_id: Uuid::nil(),
            display_name: None,
            quota_bytes: 0,
            is_active: true,
            comment: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            domain_served: Some(true),
            owner_known: false,
            has_credential: false,
            owner_message_count: 0,
            used_bytes: None,
        };
        let json = serde_json::to_string(&view).expect("sérialisable");
        assert!(!json.contains("password"));
        assert!(!json.contains("scram"));
        // The byte figure stays null as long as no size is stored.
        assert!(json.contains("\"used_bytes\":null"));
    }
}
