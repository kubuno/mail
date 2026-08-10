package com.kubuno.mail.ui

import android.webkit.WebSettings
import android.webkit.WebView
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.Reply
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.MarkEmailUnread
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.kubuno.mail.net.AttachmentDto
import com.kubuno.mail.net.EmailMessageDto

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ThreadReaderScreen(
    state: ReaderState,
    snackbar: SnackbarHostState,
    onBack: () -> Unit,
    onArchive: () -> Unit,
    onTrash: () -> Unit,
    onReply: (ComposePrefill) -> Unit,
    onMarkUnread: () -> Unit,
    onOpenAttachment: (messageId: String, index: Int, name: String, mime: String?) -> Unit,
) {
    Scaffold(
        // The reader overlays the shell, so it hosts its own snackbar; otherwise
        // download feedback would show behind it, unseen.
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = {},
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Retour")
                    }
                },
                actions = {
                    if (state is ReaderState.Loaded) {
                        IconButton(onClick = { onReply(replyPrefill(state)); onBack() }) {
                            Icon(Icons.AutoMirrored.Outlined.Reply, contentDescription = "Répondre")
                        }
                    }
                    IconButton(onClick = { onArchive(); onBack() }) {
                        Icon(Icons.Outlined.Archive, contentDescription = "Archiver")
                    }
                    IconButton(onClick = { onTrash(); onBack() }) {
                        Icon(Icons.Outlined.Delete, contentDescription = "Supprimer")
                    }
                    IconButton(onClick = { onMarkUnread(); onBack() }) {
                        Icon(Icons.Outlined.MarkEmailUnread, contentDescription = "Marquer non lu")
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            when (state) {
                is ReaderState.Loading -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                is ReaderState.Failed -> Text(
                    "Impossible d'ouvrir la conversation",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.align(Alignment.Center),
                )
                is ReaderState.Loaded -> Column(
                    Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
                ) {
                    Text(
                        state.thread.subject?.takeIf { it.isNotBlank() } ?: "(sans objet)",
                        style = MaterialTheme.typography.titleLarge,
                        color = MaterialTheme.colorScheme.onSurface,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
                    )
                    state.messages.forEach { message ->
                        HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
                        MessageCard(message, onOpenAttachment)
                    }
                }
            }
        }
    }
}

@Composable
private fun MessageCard(
    message: EmailMessageDto,
    onOpenAttachment: (messageId: String, index: Int, name: String, mime: String?) -> Unit,
) {
    val sender = (message.fromName?.takeIf { it.isNotBlank() } ?: message.fromEmail).orEmpty()
    Column(Modifier.fillMaxWidth().padding(16.dp)) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Avatar40(sender)
            Column(Modifier.weight(1f)) {
                Text(
                    sender.ifBlank { "(inconnu)" },
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = MaterialTheme.colorScheme.onSurface,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                message.fromEmail?.let {
                    Text(
                        it,
                        fontSize = 12.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
        Box(Modifier.padding(top = 12.dp)) {
            val html = message.bodyHtml?.takeIf { it.isNotBlank() }
            if (html != null) HtmlBody(html)
            else Text(
                message.bodyText.orEmpty(),
                fontSize = 14.sp,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }
        message.attachments.forEachIndexed { index, attachment ->
            AttachmentChip(attachment) {
                onOpenAttachment(message.id, index, attachment.display, attachment.mime)
            }
        }
    }
}

/** A tappable attachment: name and size, downloaded and opened on click. */
@Composable
private fun AttachmentChip(attachment: AttachmentDto, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .padding(top = 8.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(
            Icons.Outlined.AttachFile,
            contentDescription = null,
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.size(18.dp),
        )
        Column {
            Text(
                attachment.display,
                fontSize = 14.sp,
                color = MaterialTheme.colorScheme.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (attachment.size > 0) {
                Text(
                    formatBytes(attachment.size),
                    fontSize = 12.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

private fun formatBytes(bytes: Long): String = when {
    bytes >= 1_048_576 -> "%.1f Mo".format(bytes / 1_048_576.0)
    bytes >= 1024 -> "%.0f Ko".format(bytes / 1024.0)
    else -> "$bytes o"
}

/**
 * Renders an email body in a locked-down WebView: JavaScript off, no file or
 * content access, and remote images blocked by default (the web loads them
 * eagerly; a mobile client should not leak read-receipts). The body is wrapped
 * in a base stylesheet so it fits the column and long tables scroll inside the
 * message rather than the page. Server-side sanitising still applies.
 */
@Composable
private fun HtmlBody(html: String) {
    val textColor = MaterialTheme.colorScheme.onSurface.toArgb()
    AndroidView(
        modifier = Modifier.fillMaxWidth(),
        factory = { context ->
            WebView(context).apply {
                settings.javaScriptEnabled = false
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                settings.loadsImagesAutomatically = true
                settings.blockNetworkImage = true // remote images blocked by default
                settings.cacheMode = WebSettings.LOAD_NO_CACHE
                setBackgroundColor(Color.Transparent.toArgb())
            }
        },
        update = { web ->
            val css = """
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <style>
                  :root { color-scheme: light dark; }
                  body { margin:0; font-family:sans-serif; font-size:14px; line-height:1.5;
                         color:${colorHex(textColor)}; word-wrap:break-word; }
                  img { max-width:100%; height:auto; }
                  table { max-width:100%; }
                  pre, code { white-space:pre-wrap; word-wrap:break-word; }
                  a { color:#1a73e8; }
                  blockquote { border-left:2px solid #dadce0; margin:0; padding-left:12px; color:#5f6368; }
                </style>
            """.trimIndent()
            web.loadDataWithBaseURL(null, css + html, "text/html", "UTF-8", null)
        },
    )
}

/** A reply to the last message: its sender, an "Re:" subject, the message id. */
private fun replyPrefill(state: ReaderState.Loaded): ComposePrefill {
    val last = state.messages.lastOrNull()
    val subject = state.thread.subject.orEmpty()
    val reSubject = if (subject.startsWith("Re:", ignoreCase = true)) subject else "Re: $subject"
    return ComposePrefill(
        to = last?.fromEmail.orEmpty(),
        subject = reSubject,
        body = "",
        replyToId = last?.id,
    )
}

private fun colorHex(argb: Int): String = String.format("#%06X", 0xFFFFFF and argb)

@Composable
private fun Avatar40(label: String) {
    val color = readerAvatarColor(label)
    Box(
        modifier = Modifier.size(40.dp).clip(CircleShape).background(color),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?",
            color = Color.White,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

private val READER_AVATAR_COLORS = listOf(
    Color(0xFF1A73E8), Color(0xFFD93025), Color(0xFF188038), Color(0xFFE37400),
    Color(0xFF8430CE), Color(0xFF007B83), Color(0xFFE52592), Color(0xFF185ABC),
    Color(0xFF137333), Color(0xFFC5221F),
)

private fun readerAvatarColor(s: String): Color {
    var h = 0
    for (c in s) h = h * 31 + c.code
    return READER_AVATAR_COLORS[(if (h < 0) -h else h) % READER_AVATAR_COLORS.size]
}
