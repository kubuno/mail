package com.kubuno.mail.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.data.MailRepository
import com.kubuno.mail.net.AddressInput
import com.kubuno.mail.net.AttachmentInput
import com.kubuno.mail.net.SendBody
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject

sealed interface ComposeStatus {
    data object Idle : ComposeStatus
    data object Sending : ComposeStatus
    data object Sent : ComposeStatus
    data class Failed(val message: String) : ComposeStatus
}

/** What a reply/forward/mailto/share pre-fills; empty for a blank message. */
data class ComposePrefill(
    val to: String = "",
    val cc: String = "",
    val bcc: String = "",
    val subject: String = "",
    val body: String = "",
    val replyToId: String? = null,
    /** Files shared into the app, read into attachments when the composer opens. */
    val attachmentUris: List<android.net.Uri> = emptyList(),
)

/**
 * Sends a message as the active shared account. The From is the account's
 * default mail identity, resolved server-side from GET /mail/accounts.
 */
@HiltViewModel
class ComposeViewModel @Inject constructor(
    sharedAccounts: SharedAccounts,
    private val repo: MailRepository,
) : ViewModel() {

    private val account: SharedAccount? = sharedAccounts.list().firstOrNull()

    private val _status = MutableStateFlow<ComposeStatus>(ComposeStatus.Idle)
    val status: StateFlow<ComposeStatus> = _status.asStateFlow()

    fun send(
        to: String,
        cc: String,
        bcc: String,
        subject: String,
        body: String,
        replyToId: String?,
        attachments: List<AttachmentInput>,
        idempotencyKey: String,
    ) {
        val account = account ?: return
        _status.value = ComposeStatus.Sending
        viewModelScope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    val accountId = repo.sendingAccountId(account)
                        ?: error("Aucun compte d'envoi")
                    repo.send(
                        account = account,
                        idempotencyKey = idempotencyKey,
                        body = SendBody(
                            accountId = accountId,
                            toAddresses = parseAddresses(to),
                            ccAddresses = parseAddresses(cc),
                            bccAddresses = parseAddresses(bcc),
                            subject = subject.trim(),
                            bodyHtml = textToHtml(body),
                            replyToId = replyToId,
                            attachments = attachments,
                        ),
                    )
                }
            }
            _status.value = result.fold(
                onSuccess = { ComposeStatus.Sent },
                onFailure = { ComposeStatus.Failed(it.message ?: "Échec de l'envoi") },
            )
        }
    }

    fun reset() {
        _status.value = ComposeStatus.Idle
    }
}

/** Splits a recipients field on the usual separators into address inputs. */
internal fun parseAddresses(raw: String): List<AddressInput> =
    raw.split(',', ';', ' ', '\n')
        .map { it.trim() }
        .filter { it.contains('@') }
        .map { AddressInput(email = it) }

/**
 * Plain text to a safe HTML body: escape the markup characters, then turn
 * newlines into breaks. The server sanitises again, but sending escaped HTML
 * keeps the text intact regardless.
 */
internal fun textToHtml(text: String): String = text
    .replace("&", "&amp;")
    .replace("<", "&lt;")
    .replace(">", "&gt;")
    .replace("\n", "<br>")
