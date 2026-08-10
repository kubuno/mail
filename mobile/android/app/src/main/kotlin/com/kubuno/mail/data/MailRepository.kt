package com.kubuno.mail.data

import com.kubuno.android.account.SharedAccount
import com.kubuno.mail.net.DraftDto
import com.kubuno.mail.net.MailClients
import com.kubuno.mail.net.MoveBody
import com.kubuno.mail.net.ReadBody
import com.kubuno.mail.net.ThreadDetailDto
import com.kubuno.mail.net.ThreadDto
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.flow.Flow

/**
 * Offline-first inbox: the UI reads threads from Room, the network only feeds
 * the cache. A refresh replaces the folder's cached page (delta via /changes
 * lands in M2 once the module is deployed). Actions update Room optimistically,
 * then call the server; a failure reloads the truth from the network.
 */
@Singleton
class MailRepository @Inject constructor(
    private val clients: MailClients,
    private val db: MailDatabase,
) {
    private val dao = db.dao()

    fun threads(account: SharedAccount, folder: MailFolder): Flow<List<ThreadEntity>> =
        dao.observe(account.key, folder.key)

    /** Pulls one tab from the server and swaps it into the cache. */
    suspend fun refresh(account: SharedAccount, folder: MailFolder) {
        val api = clients.api(account)
        val rows = when (folder) {
            MailFolder.INBOX -> api.threads(folder = "inbox", limit = 50).threads
                .map { it.toEntity(account.key, folder.key) }
            MailFolder.SENT -> api.threads(folder = "sent", limit = 50).threads
                .map { it.toEntity(account.key, folder.key) }
            // Starred spans every folder, so ask by flag rather than by folder.
            MailFolder.STARRED -> api.threads(folder = "all", starred = true, limit = 50).threads
                .map { it.toEntity(account.key, folder.key) }
            MailFolder.DRAFTS -> api.drafts().drafts
                .map { it.toEntity(account.key) }
        }
        dao.clearFolder(account.key, folder.key)
        dao.upsert(rows)
    }

    /** Archive: leaves the folder, so the row disappears locally right away. */
    suspend fun archive(account: SharedAccount, id: String, folder: MailFolder) {
        dao.delete(account.key, id)
        runCatching { clients.api(account).move(id, MoveBody("archive")) }
            .onFailure { refresh(account, folder) }
    }

    suspend fun trash(account: SharedAccount, id: String, folder: MailFolder) {
        dao.delete(account.key, id)
        runCatching { clients.api(account).trash(id) }
            .onFailure { refresh(account, folder) }
    }

    /** Loads a thread with its messages (does not mark read server-side). */
    suspend fun thread(account: SharedAccount, id: String): ThreadDetailDto =
        clients.api(account).thread(id)

    /** Marks a thread read: clears the local badge, then tells the server. */
    suspend fun markRead(account: SharedAccount, id: String) {
        dao.setUnread(account.key, id, 0)
        runCatching { clients.api(account).setRead(id, ReadBody(isRead = true)) }
    }
}

/** serverUrl|userId: stable across renames, unique per account on the device. */
val SharedAccount.key: String get() = "$serverUrl|$userId"

/** A draft rendered as a row: keyed like a thread, shown by recipient. */
private fun DraftDto.toEntity(accountKey: String) = ThreadEntity(
    accountKey = accountKey,
    id = id,
    folder = MailFolder.DRAFTS.key,
    subject = subject,
    snippet = null,
    senderName = recipient,
    senderEmail = null,
    orderKey = updatedAt,
    unreadCount = 0,
    isStarred = false,
    isImportant = false,
    hasAttachments = false,
    category = null,
)

private fun ThreadDto.toEntity(accountKey: String, folder: String) = ThreadEntity(
    accountKey = accountKey,
    id = id,
    folder = folder,
    subject = subject,
    snippet = snippet,
    senderName = lastSenderName,
    senderEmail = lastSenderEmail,
    orderKey = lastMessageAt,
    unreadCount = unreadCount,
    isStarred = isStarred,
    isImportant = isImportant,
    hasAttachments = hasAttachments,
    category = category,
)
