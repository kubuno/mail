package com.kubuno.mail.net

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * The mail module's wire types, transcribed from the Rust structs
 * (mail/src/models/mod.rs) and the confirmed /threads and /changes contracts.
 * Only the fields the client uses are declared; unknown ones are ignored.
 */

@Serializable
data class ThreadDto(
    val id: String,
    @SerialName("account_id") val accountId: String? = null,
    val subject: String? = null,
    @SerialName("message_count") val messageCount: Int = 0,
    @SerialName("unread_count") val unreadCount: Int = 0,
    @SerialName("has_attachments") val hasAttachments: Boolean = false,
    @SerialName("is_starred") val isStarred: Boolean = false,
    @SerialName("is_important") val isImportant: Boolean = false,
    val snippet: String? = null,
    @SerialName("last_sender_name") val lastSenderName: String? = null,
    @SerialName("last_sender_email") val lastSenderEmail: String? = null,
    // Present once the server exposes it; used to show the recipient in Sent.
    @SerialName("last_recipient_name") val lastRecipientName: String? = null,
    @SerialName("last_recipient_email") val lastRecipientEmail: String? = null,
    @SerialName("last_message_at") val lastMessageAt: String? = null,
    val labels: List<LabelRef> = emptyList(),
    /** Folders this thread currently belongs to (inbox, sent, archive…). */
    val folders: List<String> = emptyList(),
    val category: String? = null,
) {
    val unread: Boolean get() = unreadCount > 0

    /** What the row shows as the correspondent: a name if we have one. */
    val senderDisplay: String
        get() = lastSenderName?.takeIf { it.isNotBlank() } ?: lastSenderEmail.orEmpty()

    /** The recipient, for the Sent view; blank until the server provides it. */
    val recipientDisplay: String
        get() = lastRecipientName?.takeIf { it.isNotBlank() } ?: lastRecipientEmail.orEmpty()
}

@Serializable
data class LabelRef(
    val id: String,
    val name: String,
    val color: String? = null,
)

@Serializable
data class ThreadPageDto(
    val threads: List<ThreadDto> = emptyList(),
    @SerialName("has_more") val hasMore: Boolean = false,
    val cursor: String? = null,
    val total: Int? = null,
)

@Serializable
data class MailAccountDto(
    val id: String,
    val name: String? = null,
    @SerialName("email_address") val emailAddress: String? = null,
    @SerialName("is_default") val isDefault: Boolean = false,
    @SerialName("is_active") val isActive: Boolean = true,
)

@Serializable
data class MailAccountsDto(
    val accounts: List<MailAccountDto> = emptyList(),
)

@Serializable
data class DraftDto(
    val id: String,
    val subject: String? = null,
    @SerialName("body_html") val bodyHtml: String? = null,
    @SerialName("to_addresses") val toAddresses: List<EmailAddressDto> = emptyList(),
    @SerialName("updated_at") val updatedAt: String? = null,
) {
    /** Drafts list by recipient, since the sender is always us. */
    val recipient: String
        get() = toAddresses.firstOrNull()?.let { it.name ?: it.email } ?: "(sans destinataire)"
}

@Serializable
data class DraftsDto(
    val drafts: List<DraftDto> = emptyList(),
)

@Serializable
data class EmailAddressDto(
    val name: String? = null,
    val email: String,
)

@Serializable
data class AttachmentDto(
    val name: String? = null,
    val filename: String? = null,
    val mime: String? = null,
    val size: Long = 0,
) {
    val display: String get() = (name ?: filename)?.takeIf { it.isNotBlank() } ?: "pièce jointe"
}

@Serializable
data class EmailMessageDto(
    val id: String,
    @SerialName("thread_id") val threadId: String? = null,
    @SerialName("from_name") val fromName: String? = null,
    @SerialName("from_email") val fromEmail: String? = null,
    @SerialName("to_addresses") val toAddresses: List<EmailAddressDto> = emptyList(),
    val subject: String? = null,
    @SerialName("body_text") val bodyText: String? = null,
    @SerialName("body_html") val bodyHtml: String? = null,
    @SerialName("is_read") val isRead: Boolean = true,
    @SerialName("is_starred") val isStarred: Boolean = false,
    @SerialName("received_at") val receivedAt: String? = null,
    val attachments: List<AttachmentDto> = emptyList(),
)

@Serializable
data class ThreadDetailDto(
    val thread: ThreadDto,
    val messages: List<EmailMessageDto> = emptyList(),
)

@Serializable
data class AddressInput(val name: String? = null, val email: String)

@Serializable
data class AttachmentInput(
    val filename: String,
    val mime: String,
    /** Base64 (standard) of the file bytes; the v1 send carries them inline. */
    val content: String,
)

@Serializable
data class SendBody(
    @SerialName("account_id") val accountId: String,
    @SerialName("to_addresses") val toAddresses: List<AddressInput>,
    @SerialName("cc_addresses") val ccAddresses: List<AddressInput> = emptyList(),
    @SerialName("bcc_addresses") val bccAddresses: List<AddressInput> = emptyList(),
    val subject: String,
    @SerialName("body_html") val bodyHtml: String,
    @SerialName("reply_to_id") val replyToId: String? = null,
    val attachments: List<AttachmentInput> = emptyList(),
)

@Serializable
data class MoveBody(val folder: String)

@Serializable
data class ReadBody(@SerialName("is_read") val isRead: Boolean)

@Serializable
data class StarResult(@SerialName("is_starred") val isStarred: Boolean = false)

/** The delta response — same thread shape, keyed by the modseq cursor. */
@Serializable
data class ChangesDto(
    val threads: List<ThreadDto> = emptyList(),
    @SerialName("deleted_ids") val deletedIds: List<String> = emptyList(),
    val cursor: String? = null,
    @SerialName("has_more") val hasMore: Boolean = false,
)
