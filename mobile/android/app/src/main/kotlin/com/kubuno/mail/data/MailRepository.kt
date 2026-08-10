package com.kubuno.mail.data

import com.kubuno.android.account.SharedAccount
import com.kubuno.mail.net.ChangesDto
import com.kubuno.mail.net.DraftDto
import com.kubuno.mail.net.MailClients
import com.kubuno.mail.net.MoveBody
import com.kubuno.mail.net.ReadBody
import com.kubuno.mail.net.SendBody
import com.kubuno.mail.net.ThreadDetailDto
import com.kubuno.mail.net.ThreadDto
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.withContext

/**
 * Offline-first mailbox. The UI reads threads from Room; the network only
 * feeds the cache.
 *
 * The three thread folders (inbox, sent, starred) are kept by the /changes
 * delta: each sync fetches only what moved since the stored modseq cursor and
 * reconciles every view from the thread's current folder membership and star
 * flag. Drafts live in a separate table server-side, so they keep their own
 * full fetch. Actions update Room optimistically and roll back on failure.
 */
@Singleton
class MailRepository @Inject constructor(
    @dagger.hilt.android.qualifiers.ApplicationContext private val context: android.content.Context,
    private val clients: MailClients,
    private val db: MailDatabase,
) {
    private val dao = db.dao()

    // Folders reconciled from the delta, with how membership is decided.
    private val deltaFolders = listOf(
        MailFolder.INBOX to { t: ThreadDto -> t.folders.contains("inbox") },
        MailFolder.SENT to { t: ThreadDto -> t.folders.contains("sent") },
        MailFolder.STARRED to { t: ThreadDto -> t.isStarred },
    )

    fun threads(account: SharedAccount, folder: MailFolder): Flow<List<ThreadEntity>> =
        dao.observe(account.key, folder.key)

    /** Refreshes one tab: drafts by their own endpoint, the rest by delta. */
    suspend fun refresh(account: SharedAccount, folder: MailFolder) {
        if (folder == MailFolder.DRAFTS) {
            val drafts = clients.api(account).drafts().drafts.map { it.toEntity(account.key) }
            dao.clearFolder(account.key, MailFolder.DRAFTS.key)
            dao.upsert(drafts)
        } else {
            syncDelta(account)
        }
    }

    /**
     * Incremental sync: pulls /changes from the stored cursor until drained,
     * reconciling the delta folders and dropping deleted threads. The first run
     * (cursor 0) sweeps the history and populates the caches.
     */
    suspend fun syncDelta(account: SharedAccount) {
        val api = clients.api(account)
        var cursor = dao.cursor(account.key) ?: "0"
        while (true) {
            val res = api.changes(since = cursor.toLongOrNull() ?: 0L, limit = 200)
            apply(account.key, res)
            cursor = res.cursor ?: cursor
            dao.setCursor(SyncStateEntity(account.key, cursor))
            if (!res.hasMore) break
        }
    }

    private suspend fun apply(accountKey: String, res: ChangesDto) {
        for (thread in res.threads) {
            for ((folder, isMember) in deltaFolders) {
                if (isMember(thread)) {
                    dao.upsert(listOf(thread.toEntity(accountKey, folder.key)))
                } else {
                    dao.deleteInFolder(accountKey, folder.key, thread.id)
                }
            }
        }
        for (id in res.deletedIds) dao.delete(accountKey, id)
    }

    /** Archive: leaves the inbox, so the row disappears locally right away. */
    suspend fun archive(account: SharedAccount, id: String) = act(account, id) {
        clients.api(account).move(id, MoveBody("archive"))
    }

    suspend fun trash(account: SharedAccount, id: String) = act(account, id) {
        clients.api(account).trash(id)
    }

    /** Optimistic remove-then-call, restoring the exact rows on failure. */
    private suspend fun act(account: SharedAccount, id: String, call: suspend () -> Unit) {
        val saved = dao.rowsFor(account.key, id)
        dao.delete(account.key, id)
        runCatching { call() }.onFailure { dao.upsert(saved) }
    }

    /** Loads a thread with its messages (does not mark read server-side). */
    suspend fun thread(account: SharedAccount, id: String): ThreadDetailDto =
        clients.api(account).thread(id)

    /**
     * Downloads one attachment to the app cache and returns the file. Streams
     * over the authenticated client — the same endpoint that now speaks Range,
     * so a resumed download would work, though here we fetch it whole.
     */
    suspend fun downloadAttachment(
        account: SharedAccount,
        messageId: String,
        index: Int,
        filename: String,
    ): java.io.File = withContext(kotlinx.coroutines.Dispatchers.IO) {
        val client = clients.raw(account)
        val url = "${client.serverUrl}/api/v1/mail/messages/$messageId/attachments/$index"
        val request = okhttp3.Request.Builder().url(url).build()
        client.okHttpClient.newCall(request).execute().use { response ->
            if (!response.isSuccessful) error("HTTP ${response.code}")
            val dir = java.io.File(context.cacheDir, "attachments").apply { mkdirs() }
            val safe = filename.replace(Regex("[^A-Za-z0-9._-]"), "_").ifBlank { "attachment" }
            val file = java.io.File(dir, safe)
            response.body?.byteStream()?.use { input ->
                file.outputStream().use { input.copyTo(it) }
            } ?: error("Réponse vide")
            file
        }
    }

    /** Marks a thread read: clears the local badge, then tells the server. */
    suspend fun markRead(account: SharedAccount, id: String) {
        dao.setUnread(account.key, id, 0)
        runCatching { clients.api(account).setRead(id, ReadBody(isRead = true)) }
    }

    /** Marks a thread unread again (a fresh badge of 1). */
    suspend fun markUnread(account: SharedAccount, id: String) {
        dao.setUnread(account.key, id, 1)
        runCatching { clients.api(account).setRead(id, ReadBody(isRead = false)) }
    }

    /** Toggles the star: flips it in the cache first, reverts if the call fails. */
    suspend fun toggleStar(account: SharedAccount, id: String) {
        val wasStarred = dao.rowsFor(account.key, id).firstOrNull()?.isStarred ?: false
        dao.setStarred(account.key, id, !wasStarred)
        runCatching { clients.api(account).toggleStar(id) }
            .onFailure { dao.setStarred(account.key, id, wasStarred) }
    }

    /** The mail account a message is sent from: the default, else the first active. */
    suspend fun sendingAccountId(account: SharedAccount): String? {
        val accounts = clients.api(account).accounts().accounts
        return (accounts.firstOrNull { it.isDefault && it.isActive }
            ?: accounts.firstOrNull { it.isActive }
            ?: accounts.firstOrNull())?.id
    }

    /**
     * Sends a message with a stable [idempotencyKey] so a retry cannot send it
     * twice. Refreshes the Sent folder afterwards so the copy shows up.
     */
    suspend fun send(account: SharedAccount, idempotencyKey: String, body: SendBody) {
        clients.api(account).send(idempotencyKey, body)
        runCatching { refresh(account, MailFolder.SENT) }
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
