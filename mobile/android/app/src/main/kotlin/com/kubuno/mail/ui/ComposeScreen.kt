package com.kubuno.mail.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Send
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import java.util.UUID

/**
 * The composer. Plain-text body for v1 (a rich editor has no Compose
 * equivalent worth its weight yet); the send is made safe against retries by a
 * key minted once when the screen opens.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ComposeScreen(
    prefill: ComposePrefill,
    onClose: () -> Unit,
    onSent: () -> Unit,
    viewModel: ComposeViewModel = hiltViewModel(),
) {
    val status by viewModel.status.collectAsStateWithLifecycle()
    val idempotencyKey = remember { UUID.randomUUID().toString() }

    var to by remember { mutableStateOf(prefill.to) }
    var cc by remember { mutableStateOf("") }
    var bcc by remember { mutableStateOf("") }
    var showCc by remember { mutableStateOf(false) }
    var subject by remember { mutableStateOf(prefill.subject) }
    var body by remember { mutableStateOf(prefill.body) }

    // Leave the screen once the message is on its way; the inbox confirms it.
    LaunchedEffect(status) {
        if (status is ComposeStatus.Sent) {
            viewModel.reset()
            onSent()
        }
    }

    val canSend = to.contains('@') && subject.isNotBlank() && status != ComposeStatus.Sending

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Nouveau message") },
                navigationIcon = {
                    IconButton(onClick = onClose) {
                        Icon(Icons.Outlined.Close, contentDescription = "Fermer")
                    }
                },
                actions = {
                    if (status == ComposeStatus.Sending) {
                        CircularProgressIndicator(Modifier.padding(end = 16.dp).size(20.dp))
                    } else {
                        FilledIconButton(
                            enabled = canSend,
                            onClick = {
                                viewModel.send(to, cc, bcc, subject, body, prefill.replyToId, idempotencyKey)
                            },
                            modifier = Modifier.padding(end = 8.dp),
                        ) {
                            Icon(
                                Icons.AutoMirrored.Rounded.Send,
                                contentDescription = "Envoyer",
                                modifier = Modifier.size(20.dp),
                            )
                        }
                    }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState()),
        ) {
            RecipientField("À", to, { to = it }, trailing = {
                TextButton(onClick = { showCc = !showCc }) { Text("Cc/Cci") }
            })
            if (showCc) {
                HorizontalDivider()
                RecipientField("Cc", cc, { cc = it })
                HorizontalDivider()
                RecipientField("Cci", bcc, { bcc = it })
            }
            HorizontalDivider()
            OutlinedTextField(
                value = subject,
                onValueChange = { subject = it },
                placeholder = { Text("Objet") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
            )
            OutlinedTextField(
                value = body,
                onValueChange = { body = it },
                placeholder = { Text("Rédigez votre message…") },
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(12.dp),
                minLines = 8,
            )
            if (status is ComposeStatus.Failed) {
                Text(
                    (status as ComposeStatus.Failed).message,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
        }
    }
}

@Composable
private fun RecipientField(
    label: String,
    value: String,
    onValueChange: (String) -> Unit,
    trailing: @Composable (() -> Unit)? = null,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email),
        trailingIcon = trailing,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
    )
}
