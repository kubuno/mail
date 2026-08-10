package com.kubuno.mail.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.data.MailRepository
import com.kubuno.mail.net.EmailMessageDto
import com.kubuno.mail.net.ThreadDto
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
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
    sharedAccounts: SharedAccounts,
    private val repo: MailRepository,
) : ViewModel() {

    private val account: SharedAccount? = sharedAccounts.list().firstOrNull()

    private val _state = MutableStateFlow<ReaderState>(ReaderState.Loading)
    val state: StateFlow<ReaderState> = _state.asStateFlow()

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

    fun reset() {
        loadedId = null
        _state.value = ReaderState.Loading
    }
}
