package com.kubuno.mail.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.net.MailClients
import com.kubuno.mail.net.ThreadDto
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject

/** What the inbox screen renders: loading, the threads, or an error/empty note. */
sealed interface InboxState {
    data object Loading : InboxState
    data class Loaded(val threads: List<ThreadDto>) : InboxState
    data class Failed(val message: String) : InboxState
    data object NoAccount : InboxState
}

/**
 * Loads the inbox of the active shared account over the borrowed-token client.
 *
 * Read-only and network-only for now: this is M1's live proof that the mail
 * HTTP client works end to end against dev.kubuno.com. Room caching, the delta
 * cursor and the swipe actions land in M2/M3 on top of this.
 */
@HiltViewModel
class InboxViewModel @Inject constructor(
    sharedAccounts: SharedAccounts,
    private val clients: MailClients,
) : ViewModel() {

    val account: SharedAccount? = sharedAccounts.list().firstOrNull()

    private val _state = MutableStateFlow<InboxState>(
        if (account == null) InboxState.NoAccount else InboxState.Loading
    )
    val state: StateFlow<InboxState> = _state.asStateFlow()

    init {
        account?.let { load(it) }
    }

    fun refresh() {
        account?.let { load(it) }
    }

    private fun load(account: SharedAccount) = viewModelScope.launch {
        _state.value = InboxState.Loading
        val result = withContext(Dispatchers.IO) {
            runCatching { clients.api(account).threads(folder = "inbox", limit = 50).threads }
        }
        _state.value = result.fold(
            onSuccess = { InboxState.Loaded(it) },
            onFailure = { InboxState.Failed(it.message ?: "Erreur") },
        )
    }
}

@Composable
fun MailApp(viewModel: InboxViewModel = hiltViewModel()) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    InboxScreen(
        title = viewModel.account?.let { "Boîte de réception · ${it.label}" } ?: "Kubuno Mail",
        state = state,
        onRefresh = viewModel::refresh,
    )
}
