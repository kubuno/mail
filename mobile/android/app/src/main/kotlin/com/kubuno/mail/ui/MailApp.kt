package com.kubuno.mail.ui

import androidx.activity.compose.BackHandler
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.data.MailFolder
import com.kubuno.mail.data.MailRepository
import com.kubuno.mail.data.ThreadEntity
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import javax.inject.Inject

/**
 * Offline-first inbox: threads come from Room (instant, works offline), a
 * refresh feeds the cache from the server. Swipe archives or trashes a thread,
 * updating Room first so the row leaves immediately.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@HiltViewModel
class InboxViewModel @Inject constructor(
    sharedAccounts: SharedAccounts,
    private val repo: MailRepository,
) : ViewModel() {

    val account: SharedAccount? = sharedAccounts.list().firstOrNull()

    private val _folder = MutableStateFlow(MailFolder.INBOX)
    val folder: StateFlow<MailFolder> = _folder

    /** The rows follow the selected tab, straight from Room. */
    val threads: StateFlow<List<ThreadEntity>> =
        _folder.flatMapLatest { f ->
            account?.let { repo.threads(it, f) } ?: flowOf(emptyList())
        }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyList())

    private val _refreshing = MutableStateFlow(false)
    val refreshing: StateFlow<Boolean> = _refreshing

    init {
        if (account != null) refresh()
    }

    /** Switching a tab shows its cache at once, then refreshes it. */
    fun selectFolder(folder: MailFolder) {
        if (_folder.value == folder) return
        _folder.value = folder
        refresh()
    }

    fun refresh() {
        val account = account ?: return
        val folder = _folder.value
        viewModelScope.launch {
            _refreshing.value = true
            runCatching { repo.refresh(account, folder) }
            _refreshing.value = false
        }
    }

    fun archive(id: String) = account?.let { a ->
        viewModelScope.launch { repo.archive(a, id, _folder.value) }
    }

    fun trash(id: String) = account?.let { a ->
        viewModelScope.launch { repo.trash(a, id, _folder.value) }
    }
}

@Composable
fun MailApp(
    viewModel: InboxViewModel = hiltViewModel(),
    reader: ReaderViewModel = hiltViewModel(),
) {
    val threads by viewModel.threads.collectAsStateWithLifecycle()
    val refreshing by viewModel.refreshing.collectAsStateWithLifecycle()
    val folder by viewModel.folder.collectAsStateWithLifecycle()
    // The reader overlays the list — the list stays composed so its scroll
    // position and cache survive, matching the web's mobile behaviour.
    var openThread by remember { mutableStateOf<String?>(null) }

    InboxScreen(
        subtitle = viewModel.account?.label,
        hasAccount = viewModel.account != null,
        folder = folder,
        onSelectFolder = viewModel::selectFolder,
        threads = threads,
        refreshing = refreshing,
        onRefresh = viewModel::refresh,
        // Drafts open the composer (M3); for now only threads open the reader.
        onOpen = { id -> if (folder != MailFolder.DRAFTS) { openThread = id; reader.open(id) } },
        onArchive = viewModel::archive,
        onTrash = viewModel::trash,
    )

    if (openThread != null) {
        val readerState by reader.state.collectAsStateWithLifecycle()
        val id = openThread!!
        fun close() { openThread = null; reader.reset() }
        BackHandler { close() }
        ThreadReaderScreen(
            state = readerState,
            onBack = { close() },
            onArchive = { viewModel.archive(id) },
            onTrash = { viewModel.trash(id) },
        )
    }
}
