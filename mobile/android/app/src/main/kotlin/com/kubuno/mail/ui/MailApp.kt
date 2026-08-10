package com.kubuno.mail.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.data.MailRepository
import com.kubuno.mail.data.ThreadEntity
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import javax.inject.Inject

/**
 * Offline-first inbox: threads come from Room (instant, works offline), a
 * refresh feeds the cache from the server. Swipe archives or trashes a thread,
 * updating Room first so the row leaves immediately.
 */
@HiltViewModel
class InboxViewModel @Inject constructor(
    sharedAccounts: SharedAccounts,
    private val repo: MailRepository,
) : ViewModel() {

    val account: SharedAccount? = sharedAccounts.list().firstOrNull()

    val threads: StateFlow<List<ThreadEntity>> =
        (account?.let { repo.inbox(it) } ?: flowOf(emptyList()))
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyList())

    private val _refreshing = kotlinx.coroutines.flow.MutableStateFlow(false)
    val refreshing: StateFlow<Boolean> = _refreshing

    init {
        if (account != null) refresh()
    }

    fun refresh() {
        val account = account ?: return
        viewModelScope.launch {
            _refreshing.value = true
            runCatching { repo.refresh(account) }
            _refreshing.value = false
        }
    }

    fun archive(id: String) = account?.let { a ->
        viewModelScope.launch { repo.archive(a, id) }
    }

    fun trash(id: String) = account?.let { a ->
        viewModelScope.launch { repo.trash(a, id) }
    }
}

@Composable
fun MailApp(viewModel: InboxViewModel = hiltViewModel()) {
    val threads by viewModel.threads.collectAsStateWithLifecycle()
    val refreshing by viewModel.refreshing.collectAsStateWithLifecycle()
    InboxScreen(
        title = viewModel.account?.let { "Boîte de réception" } ?: "Kubuno Mail",
        subtitle = viewModel.account?.label,
        hasAccount = viewModel.account != null,
        threads = threads,
        refreshing = refreshing,
        onRefresh = viewModel::refresh,
        onArchive = viewModel::archive,
        onTrash = viewModel::trash,
    )
}
