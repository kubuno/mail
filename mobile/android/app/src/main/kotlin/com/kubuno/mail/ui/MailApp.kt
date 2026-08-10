package com.kubuno.mail.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.mail.R
import dagger.hilt.android.lifecycle.HiltViewModel
import javax.inject.Inject

/**
 * Discovers the Kubuno accounts on the device through the system
 * AccountManager — the accounts the drive app (or any sibling) signed in.
 *
 * This is the scaffold's proof that cross-app sharing reaches the mail app:
 * the per-app registry is private, so a consumer app reads the shared
 * accounts here. The real folder/thread UI, and obtaining access tokens for
 * these accounts, land on top of it next.
 */
@HiltViewModel
class MailHomeViewModel @Inject constructor(
    sharedAccounts: SharedAccounts,
) : ViewModel() {
    val accounts: List<SharedAccount> = sharedAccounts.list()
}

@Composable
fun MailApp(viewModel: MailHomeViewModel = hiltViewModel()) {
    val active = viewModel.accounts.firstOrNull()

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
        }
    }
}
