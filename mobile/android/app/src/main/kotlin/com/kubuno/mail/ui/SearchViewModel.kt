package com.kubuno.mail.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.data.MailRepository
import com.kubuno.mail.net.ThreadDto
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.launch
import javax.inject.Inject

sealed interface SearchState {
    data object Idle : SearchState
    data object Searching : SearchState
    data object Empty : SearchState
    data class Results(val threads: List<ThreadDto>) : SearchState
}

/** Debounced full-text search across the active account's folders. */
@OptIn(FlowPreview::class)
@HiltViewModel
class SearchViewModel @Inject constructor(
    sharedAccounts: SharedAccounts,
    private val repo: MailRepository,
) : ViewModel() {

    private val account: SharedAccount? = sharedAccounts.list().firstOrNull()

    private val _query = MutableStateFlow("")
    val query: StateFlow<String> = _query.asStateFlow()

    private val _state = MutableStateFlow<SearchState>(SearchState.Idle)
    val state: StateFlow<SearchState> = _state.asStateFlow()

    init {
        _query
            .debounce(250)
            .distinctUntilChanged()
            .onEach { runSearch(it.trim()) }
            .launchIn(viewModelScope)
    }

    fun onQueryChange(value: String) {
        _query.value = value
    }

    private fun runSearch(term: String) {
        val account = account
        if (term.length < 2 || account == null) {
            _state.value = SearchState.Idle
            return
        }
        _state.value = SearchState.Searching
        viewModelScope.launch {
            val result = runCatching { repo.search(account, term) }.getOrDefault(emptyList())
            _state.value = if (result.isEmpty()) SearchState.Empty else SearchState.Results(result)
        }
    }
}
