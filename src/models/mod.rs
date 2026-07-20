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
    pub incoming_protocol: String,
    pub imap_host:      String,
    pub imap_port:      i32,
    pub imap_security:  String,
    pub imap_username:  String,
    pub smtp_host:      String,
    pub smtp_port:      i32,
    pub smtp_security:  String,
    pub smtp_username:  String,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateLabelDto {
    pub account_id: Uuid,
    pub name:       String,
    pub color:      Option<String>,
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
    pub limit:      Option<i64>,
    pub before:     Option<DateTime<Utc>>,
    pub search:     Option<String>,
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
