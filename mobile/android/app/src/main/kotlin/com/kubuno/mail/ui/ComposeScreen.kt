package com.kubuno.mail.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Send
import androidx.compose.material.icons.outlined.AttachFile
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
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.kubuno.mail.net.AttachmentInput
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
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
    val attachments = remember { mutableStateListOf<PendingAttachment>() }

    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) scope.launch {
            readAttachment(context, uri)?.let { attachments.add(it) }
        }
    }

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
                    IconButton(onClick = { picker.launch(arrayOf("*/*")) }) {
                        Icon(Icons.Outlined.AttachFile, contentDescription = "Joindre un fichier")
                    }
                    if (status == ComposeStatus.Sending) {
                        CircularProgressIndicator(Modifier.padding(end = 16.dp).size(20.dp))
                    } else {
                        FilledIconButton(
                            enabled = canSend,
                            onClick = {
                                viewModel.send(
                                    to, cc, bcc, subject, body, prefill.replyToId,
                                    attachments.map { it.toInput() }, idempotencyKey,
                                )
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
            attachments.forEachIndexed { index, att ->
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Icon(
                        Icons.Outlined.AttachFile,
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.size(18.dp),
                    )
                    Text(
                        "${att.filename} · ${formatBytes(att.size)}",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurface,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = { attachments.removeAt(index) }) {
                        Icon(Icons.Outlined.Close, contentDescription = "Retirer", modifier = Modifier.size(18.dp))
                    }
                }
            }
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

/** A file chosen for sending, already read into base64 for the inline v1 send. */
data class PendingAttachment(
    val filename: String,
    val mime: String,
    val base64: String,
    val size: Long,
) {
    fun toInput() = AttachmentInput(filename = filename, mime = mime, content = base64)
}

/** Reads the picked document into a base64 attachment, off the main thread. */
private suspend fun readAttachment(
    context: android.content.Context,
    uri: android.net.Uri,
): PendingAttachment? = withContext(Dispatchers.IO) {
    runCatching {
        val resolver = context.contentResolver
        var name = "piece-jointe"
        var size = 0L
        resolver.query(uri, null, null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                val nameIdx = cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
                val sizeIdx = cursor.getColumnIndex(android.provider.OpenableColumns.SIZE)
                if (nameIdx >= 0) cursor.getString(nameIdx)?.let { name = it }
                if (sizeIdx >= 0 && !cursor.isNull(sizeIdx)) size = cursor.getLong(sizeIdx)
            }
        }
        val bytes = resolver.openInputStream(uri)?.use { it.readBytes() } ?: return@runCatching null
        val base64 = android.util.Base64.encodeToString(bytes, android.util.Base64.NO_WRAP)
        PendingAttachment(
            filename = name,
            mime = resolver.getType(uri) ?: "application/octet-stream",
            base64 = base64,
            size = if (size > 0) size else bytes.size.toLong(),
        )
    }.getOrNull()
}

private fun formatBytes(bytes: Long): String = when {
    bytes >= 1_048_576 -> "%.1f Mo".format(bytes / 1_048_576.0)
    bytes >= 1024 -> "%.0f Ko".format(bytes / 1024.0)
    else -> "$bytes o"
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
