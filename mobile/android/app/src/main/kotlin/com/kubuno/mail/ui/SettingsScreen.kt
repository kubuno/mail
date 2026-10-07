package com.kubuno.mail.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.ManageAccounts
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.kubuno.android.account.SharedAccount
import com.kubuno.mail.BuildConfig

/**
 * A minimal settings surface for now: which account is shown, a shortcut to the
 * system account manager, and the build. The web's eight tabs (filters, spam,
 * PGP…) come as those features land.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MailSettingsScreen(
    account: SharedAccount?,
    onBack: () -> Unit,
    onManageDeviceAccounts: () -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Paramètres") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Retour")
                    }
                },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            Text(
                "Compte",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 4.dp),
            )
            ListItem(
                headlineContent = { Text(account?.label ?: "Aucun compte") },
                supportingContent = { account?.let { Text("${it.email ?: it.userId} · ${it.host}") } },
            )
            HorizontalDivider()
            ListItem(
                modifier = Modifier.clickable(onClick = onManageDeviceAccounts),
                headlineContent = { Text("Gérer les comptes sur cet appareil") },
                leadingContent = {
                    Icon(Icons.Outlined.ManageAccounts, contentDescription = null)
                },
            )
            HorizontalDivider()
            Text(
                "Kubuno Mail ${BuildConfig.VERSION_NAME}",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(16.dp).fillMaxWidth(),
            )
        }
    }
}
