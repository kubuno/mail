use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct EmailAccount {
    pub id:             Uuid,
    pub user_id:        Uuid,
    pub name:           String,
    pub email_address:  String,
    /// "external" (an IMAP/SMTP account at another provider) or "local" (a hosted
    /// mailbox of this instance — the instance itself is the server).
    pub kind:           String,
    /// The hosted mailbox this account fronts, when `kind = 'local'`; NULL for an
    /// external account or once the mailbox has been deleted.
    pub mailbox_id:     Option<Uuid>,
    pub incoming_protocol: String,
    pub imap_host:      String,
    pub imap_port:      i32,
    pub imap_security:  String,
    pub imap_username:  String,
    pub smtp_host:      String,
    pub smtp_port:      i32,
    pub smtp_security:  String,
    pub smtp_username:  String,
    /// "password" | "oauth_google" | "oauth_microsoft"
    pub auth_kind:      String,
    pub is_default:     bool,
    pub is_active:      bool,
    pub last_sync_at:   Option<DateTime<Utc>>,
    pub last_error:     Option<String>,
    pub created_at:     DateTime<Utc>,
    pub updated_at:     DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAccountDto {
    pub name:              String,
    pub email_address:     String,
    pub incoming_protocol: Option<String>,
    pub imap_host:         String,
    pub imap_port:      Option<i32>,
    pub imap_security:  Option<String>,
    pub imap_username:  String,
    pub imap_password:  String,
    pub smtp_host:      String,
    pub smtp_port:      Option<i32>,
    pub smtp_security:  Option<String>,
    pub smtp_username:  String,
    pub smtp_password:  String,
    pub is_default:     Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateAccountDto {
    pub name:              Option<String>,
    pub email_address:     Option<String>,
    pub incoming_protocol: Option<String>,
    pub imap_host:         Option<String>,
    pub imap_port:      Option<i32>,
    pub imap_security:  Option<String>,
    pub imap_username:  Option<String>,
    pub imap_password:  Option<String>,
    pub smtp_host:      Option<String>,
    pub smtp_port:      Option<i32>,
    pub smtp_security:  Option<String>,
    pub smtp_username:  Option<String>,
    pub smtp_password:  Option<String>,
    pub is_default:     Option<bool>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Thread {
    pub id:              Uuid,
    pub account_id:      Uuid,
    pub user_id:         Uuid,
    pub subject:         String,
    pub message_count:   i32,
    pub unread_count:    i32,
    pub has_attachments: bool,
    pub is_starred:      bool,
    pub is_important:    bool,
    pub snippet:           Option<String>,
    pub last_sender_name:  Option<String>,
    pub last_sender_email: String,
    pub last_message_at:   DateTime<Utc>,
    pub created_at:        DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct EmailMessage {
    pub id:             Uuid,
    pub thread_id:      Uuid,
    pub account_id:     Uuid,
    pub user_id:        Uuid,
    pub message_id:     Option<String>,
    pub in_reply_to:    Option<String>,
    pub imap_uid:       Option<i64>,
    pub imap_folder:    String,
    pub from_name:      Option<String>,
    pub from_email:     String,
    pub to_addresses:   Value,
    pub cc_addresses:   Value,
    pub bcc_addresses:  Value,
    pub reply_to:       Option<String>,
    pub subject:        String,
    pub body_text:      Option<String>,
    pub body_html:      Option<String>,
    pub attachments:    Value,
    pub is_read:        bool,
    pub is_starred:     bool,
    pub is_deleted:     bool,
    pub folder:         String,
    pub label_ids:      Vec<Uuid>,
    pub sent_at:        Option<DateTime<Utc>>,
    pub received_at:    DateTime<Utc>,
    pub created_at:     DateTime<Utc>,
    pub spam_score:     Option<f32>,
    pub list_unsubscribe: Option<String>,
    /// Provenance shown in the message details panel.
    pub mailed_by: Option<String>,
    pub signed_by: Option<String>,
    pub security:  Option<String>,
    /// DMARC verdict of this message; only "pass" lets the reader show the
    /// sender's BRAND logo (see services::avatars).
    pub auth_dmarc: Option<String>,
    // ── OpenPGP verdict, computed at read time (not stored columns) ────────────
    /// The body was OpenPGP-encrypted (and, when a key was available, decrypted
    /// in place before this message was returned).
    #[sqlx(default)]
    #[serde(default)]
    pub pgp_encrypted: bool,
    /// The signature verdict when the message was signed: `Some(true)` verified,
    /// `Some(false)` present but invalid / unverifiable, `None` unsigned.
    #[sqlx(default)]
    #[serde(default)]
    pub pgp_signature_valid: Option<bool>,
    /// Fingerprint of the key a valid signature was checked against.
    #[sqlx(default)]
    #[serde(default)]
    pub pgp_signer_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Draft {
    pub id:           Uuid,
    pub account_id:   Uuid,
    pub user_id:      Uuid,
    pub to_addresses: Value,
    pub cc_addresses: Value,
    pub bcc_addresses: Value,
    pub subject:      String,
    pub body_html:    String,
    pub reply_to_id:  Option<Uuid>,
    pub attachments:  Value,
    pub created_at:   DateTime<Utc>,
    pub updated_at:   DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveDraftDto {
    pub account_id:   Uuid,
    pub to_addresses: Option<Vec<EmailAddress>>,
    pub cc_addresses: Option<Vec<EmailAddress>>,
    pub bcc_addresses: Option<Vec<EmailAddress>>,
    pub subject:      Option<String>,
    pub body_html:    Option<String>,
    pub reply_to_id:  Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendMailDto {
    pub account_id:   Uuid,
    pub to_addresses: Vec<EmailAddress>,
    pub cc_addresses: Option<Vec<EmailAddress>>,
    pub bcc_addresses: Option<Vec<EmailAddress>>,
    pub subject:      String,
    pub body_html:    String,
    pub reply_to_id:  Option<Uuid>,
    pub draft_id:     Option<Uuid>,
    pub scheduled_at: Option<DateTime<Utc>>,   // si présent → envoi programmé
    pub attachments:  Option<Vec<AttachmentInput>>,
    /// OpenPGP: sign and/or encrypt the message with PGP/MIME (see services::pgp_mime).
    #[serde(default)]
    pub sign:         Option<bool>,
    #[serde(default)]
    pub encrypt:      Option<bool>,
    /// Labels the composer picked; applied to the Sent copy's thread after send.
    /// Only ids owned by the sender are honoured (see handlers::messages).
    #[serde(default)]
    pub label_ids:    Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttachmentInput {
    pub filename: String,
    pub mime:     String,
    pub content:  String,   // base64 (standard)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailAddress {
    pub name:  Option<String>,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Label {
    pub id:          Uuid,
    pub account_id:  Uuid,
    pub user_id:     Uuid,
    pub name:        String,
    pub color:       Option<String>,
    pub imap_folder: Option<String>,
    pub is_system:   bool,
    pub position:    i32,
    pub created_at:  DateTime<Utc>,
    /// Sidebar visibility: "show" | "unread" | "hide".
    pub list_visibility: String,
    /// Chip on message rows: "show" | "hide".
    pub message_list_visibility: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateLabelDto {
    pub account_id: Uuid,
    pub name:       String,
    pub color:      Option<String>,
}

/// Partial update — every field is optional, only the ones sent are applied.
/// `color` is doubly optional so an explicit `null` can clear it.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateLabelDto {
    pub name:  Option<String>,
    #[serde(default, deserialize_with = "crate::models::double_option")]
    pub color: Option<Option<String>>,
    pub list_visibility:         Option<String>,
    pub message_list_visibility: Option<String>,
}

/// Distinguishes "field absent" from "field explicitly null" in a JSON patch.
pub fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestConnectionDto {
    pub incoming_protocol: Option<String>, // "imap" | "pop3"
    pub imap_host:         String,
    pub imap_port:         Option<i32>,
    pub imap_security:     Option<String>,
    pub imap_username:     String,
    pub imap_password:     String,
    pub smtp_host:         String,
    pub smtp_port:         Option<i32>,
    pub smtp_security:     Option<String>,
    pub smtp_username:     String,
    pub smtp_password:     String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThreadListQuery {
    pub account_id: Option<Uuid>,
    pub folder:     Option<String>,
    pub label_id:   Option<Uuid>,
    pub starred:    Option<bool>,
    pub important:  Option<bool>,
    pub snoozed:    Option<bool>,
    pub unread:     Option<bool>,
    /// Inbox tab filter: main | social | notifications | promotions.
    pub category:   Option<String>,
    /// Narrows `folder=custom` down to one of the account's own IMAP folders,
    /// named exactly as the provider spells it.
    pub imap_folder: Option<String>,
    pub limit:      Option<i64>,
    pub before:     Option<DateTime<Utc>>,
    pub search:     Option<String>,
    /// Act on another user's mailbox (account delegation). When set, the request
    /// is scoped to that grantor's mailbox, but only if an ACCEPTED delegation
    /// names the caller as delegate (enforced by `resolve_acting_user`).
    pub on_behalf_of: Option<Uuid>,
}

/// Query carrying only the optional account-delegation target, for routes whose
/// sole variable input is "act on whose mailbox?" (e.g. GET /threads/:id,
/// GET /counts). See `services::delegation::resolve_acting_user`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OnBehalfQuery {
    pub on_behalf_of: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SnoozeDto {
    pub until: Option<DateTime<Utc>>,   // None = réveiller (désnoozer)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EmailFilter {
    pub id:               Uuid,
    pub user_id:          Uuid,
    pub account_id:       Option<Uuid>,
    pub from_contains:    Option<String>,
    pub to_contains:      Option<String>,
    pub subject_contains: Option<String>,
    pub query_contains:   Option<String>,
    pub act_archive:      bool,
    pub act_mark_read:    bool,
    pub act_star:         bool,
    pub act_important:    bool,
    pub act_trash:        bool,
    pub act_spam:         bool,
    pub act_label_id:     Option<Uuid>,
    pub position:         i32,
    pub created_at:       DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateFilterDto {
    pub account_id:       Option<Uuid>,
    pub from_contains:    Option<String>,
    pub to_contains:      Option<String>,
    pub subject_contains: Option<String>,
    pub query_contains:   Option<String>,
    pub act_archive:      Option<bool>,
    pub act_mark_read:    Option<bool>,
    pub act_star:         Option<bool>,
    pub act_important:    Option<bool>,
    pub act_trash:        Option<bool>,
    pub act_spam:         Option<bool>,
    pub act_label_id:     Option<Uuid>,
    pub apply_existing:   Option<bool>,   // appliquer aussi aux messages déjà reçus
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct BlockedSender {
    pub id:         Uuid,
    pub email:      String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlockSenderDto {
    pub email: String,
}
