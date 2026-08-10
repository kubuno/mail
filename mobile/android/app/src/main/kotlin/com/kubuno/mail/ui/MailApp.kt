package com.kubuno.mail.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.BrokeredClients
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.R
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.Request
import javax.inject.Inject

/**
 * Discovers the shared Kubuno accounts and, as a proof that the borrowed-token
 * path works end to end, makes one real authenticated call (`GET /api/v1/me`)
 * as the active account — with a token borrowed from the owning app through
 * the system AccountManager, no refresh token in this process.
 *
 * The folder/thread UI replaces this probe next; the plumbing under it stays.
 */
@HiltViewModel
class MailHomeViewModel @Inject constructor(
    sharedAccounts: SharedAccounts,
    private val brokered: BrokeredClients,
) : ViewModel() {

    val accounts: List<SharedAccount> = sharedAccounts.list()

    private val _status = MutableStateFlow("…")
    val status: StateFlow<String> = _status

    init {
        val account = accounts.firstOrNull()
        if (account == null) {
            _status.value = ""
        } else {
            probe(account)
        }
    }

    /** GET /api/v1/me as [account], authenticated by a borrowed access token. */
    private fun probe(account: SharedAccount) = viewModelScope.launch {
        val result = withContext(Dispatchers.IO) {
            runCatching {
                val client = brokered.of(account)
                val request = Request.Builder()
                    .url(client.serverUrl + "/api/v1/me")
                    .build()
                client.okHttpClient.newCall(request).execute().use { it.code }
            }
        }
        _status.value = result.fold(
            onSuccess = { code -> if (code in 200..299) "Connecté ✓ (HTTP $code)" else "HTTP $code" },
            onFailure = { "Erreur réseau" },
        )
    }
}

@Composable
fun MailApp(viewModel: MailHomeViewModel = hiltViewModel()) {
    val active = viewModel.accounts.firstOrNull()
    val status by viewModel.status.collectAsStateWithLifecycle()

    Scaffold { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(24.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                stringResource(R.string.mail_scaffold_ready),
                style = MaterialTheme.typography.headlineSmall,
                color = MaterialTheme.colorScheme.onSurface,
            )
            Text(
                active?.let { "${it.label} · ${it.host}" }
                    ?: stringResource(R.string.mail_no_account),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(top = 8.dp),
            )
            if (active != null) {
                Text(
                    status,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        }
    }
}
