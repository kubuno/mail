package com.kubuno.mail.ui

import android.content.Context
import android.content.Intent
import androidx.core.content.FileProvider
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.data.MailRepository
import com.kubuno.mail.net.EmailMessageDto
import com.kubuno.mail.net.ThreadDto
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject

sealed interface ReaderState {
    data object Loading : ReaderState
    data class Loaded(val thread: ThreadDto, val messages: List<EmailMessageDto>) : ReaderState
    data object Failed : ReaderState
}

/**
 * Loads one thread's messages on demand. Opening a thread marks it read (the
 * server's GET does not), so the inbox badge clears the moment it is opened.
 */
@HiltViewModel
class ReaderViewModel @Inject constructor(
    @ApplicationContext private val context: Context,
    sharedAccounts: SharedAccounts,
    private val repo: MailRepository,
) : ViewModel() {

    private val account: SharedAccount? = sharedAccounts.list().firstOrNull()

    private val _state = MutableStateFlow<ReaderState>(ReaderState.Loading)
    val state: StateFlow<ReaderState> = _state.asStateFlow()

    /** One-shot messages for the shell to surface (download outcome). */
    private val _events = MutableSharedFlow<String>(extraBufferCapacity = 4)
    val events: SharedFlow<String> = _events.asSharedFlow()

    private var loadedId: String? = null

    fun open(threadId: String) {
        if (loadedId == threadId) return
        loadedId = threadId
        val account = account ?: return
        _state.value = ReaderState.Loading
        viewModelScope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching { repo.thread(account, threadId) }
            }
            _state.value = result.fold(
                onSuccess = { ReaderState.Loaded(it.thread, it.messages) },
                onFailure = { ReaderState.Failed },
            )
            // Best-effort, after the content is on screen.
            launch { repo.markRead(account, threadId) }
        }
    }

    /**
     * Downloads an attachment over the authenticated client, then opens it with
     * whatever app handles its type — via a FileProvider URI, since a raw file
     * path cannot be shared with another app.
     */
    fun openAttachment(messageId: String, index: Int, name: String, mime: String?) {
        val account = account ?: return
        viewModelScope.launch {
            _events.tryEmit("Téléchargement de $name…")
            val result = runCatching { repo.downloadAttachment(account, messageId, index, name) }
            result.onSuccess { file ->
                runCatching {
                    val uri = FileProvider.getUriForFile(
                        context, "${context.packageName}.fileprovider", file,
                    )
                    val intent = Intent(Intent.ACTION_VIEW).apply {
                        setDataAndType(uri, mime ?: "*/*")
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                    context.startActivity(intent)
                }.onFailure { _events.tryEmit("Aucune application pour ouvrir $name") }
            }.onFailure {
                _events.tryEmit("Échec du téléchargement de $name")
            }
        }
    }

    fun reset() {
        loadedId = null
        _state.value = ReaderState.Loading
    }
}
