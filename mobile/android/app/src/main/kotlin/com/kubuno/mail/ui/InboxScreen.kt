package com.kubuno.mail.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.StarBorder
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.kubuno.mail.net.ThreadDto
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InboxScreen(
    title: String,
    state: InboxState,
    onRefresh: () -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                actions = {
                    IconButton(onClick = onRefresh) {
                        Icon(Icons.Outlined.Refresh, contentDescription = "Rafraîchir")
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            when (state) {
                is InboxState.Loading -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                is InboxState.NoAccount -> Centered("Aucun compte email configuré")
                is InboxState.Failed -> Centered("Impossible de charger la boîte de réception")
                is InboxState.Loaded ->
                    if (state.threads.isEmpty()) {
                        Centered("Aucun message", Icons.Outlined.Inbox)
                    } else {
                        LazyColumn(Modifier.fillMaxSize()) {
                            items(state.threads, key = { it.id }) { ThreadRow(it) }
                        }
                    }
            }
        }
    }
}

@Composable
private fun Centered(message: String, icon: androidx.compose.ui.graphics.vector.ImageVector? = null) {
    Column(
        modifier = Modifier.fillMaxSize().padding(24.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (icon != null) {
            Icon(
                icon,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(36.dp).padding(bottom = 12.dp),
            )
        }
        Text(message, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/**
 * The two-line message row from the web's mobile design (ThreadItem.tsx): a
 * 40dp avatar, the correspondent and date on top, the subject with attachment
 * and star below, then a snippet. Unread rows are bold. 64dp min height, a
 * thumb-sized star. Swipe actions and pull-to-refresh come with M2.
 */
@Composable
private fun ThreadRow(thread: ThreadDto) {
    val unread = thread.unread
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 64.dp)
            .clickable { /* reader lands in M2 */ }
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Avatar(thread.senderDisplay)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    thread.senderDisplay.ifBlank { "(inconnu)" },
                    fontSize = 15.sp,
                    fontWeight = if (unread) FontWeight.SemiBold else FontWeight.Normal,
                    color = MaterialTheme.colorScheme.onSurface,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    formatDate(thread.lastMessageAt),
                    fontSize = 12.sp,
                    fontWeight = if (unread) FontWeight.SemiBold else FontWeight.Normal,
                    color = if (unread) MaterialTheme.colorScheme.primary
                    else MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                )
            }
            Text(
                thread.subject?.takeIf { it.isNotBlank() } ?: "(sans objet)",
                fontSize = 14.sp,
                fontWeight = if (unread) FontWeight.SemiBold else FontWeight.Normal,
                color = MaterialTheme.colorScheme.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            thread.snippet?.takeIf { it.isNotBlank() }?.let {
                Text(
                    it,
                    fontSize = 12.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Icon(
            if (thread.isStarred) Icons.Filled.Star else Icons.Outlined.StarBorder,
            contentDescription = null,
            tint = if (thread.isStarred) Color(0xFFF9AB00) else MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.size(24.dp),
        )
    }
}

/** Initial on a colour hashed from the address — the web's 10-colour palette. */
@Composable
private fun Avatar(label: String) {
    val color = AVATAR_COLORS[(hashOf(label) % AVATAR_COLORS.size)]
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

private val AVATAR_COLORS = listOf(
    Color(0xFF1A73E8), Color(0xFFD93025), Color(0xFF188038), Color(0xFFE37400),
    Color(0xFF8430CE), Color(0xFF007B83), Color(0xFFE52592), Color(0xFF185ABC),
    Color(0xFF137333), Color(0xFFC5221F),
)

private fun hashOf(s: String): Int {
    var h = 0
    for (c in s) h = h * 31 + c.code
    return if (h < 0) -h else h
}

private val TIME_FMT = DateTimeFormatter.ofPattern("HH:mm", Locale.FRANCE)
private val DAY_FMT = DateTimeFormatter.ofPattern("d MMM", Locale.FRANCE)

/** Time if it arrived today, otherwise the day — a compact echo of the web. */
private fun formatDate(iso: String?): String {
    iso ?: return ""
    return runCatching {
        val instant = Instant.parse(iso)
        val zoned = instant.atZone(ZoneId.systemDefault())
        val now = Instant.now().atZone(ZoneId.systemDefault())
        if (zoned.toLocalDate() == now.toLocalDate()) TIME_FMT.format(zoned)
        else DAY_FMT.format(zoned)
    }.getOrDefault("")
}
