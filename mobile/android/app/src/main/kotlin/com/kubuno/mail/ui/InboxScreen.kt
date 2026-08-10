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
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.StarBorder
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.kubuno.mail.data.ThreadEntity
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InboxScreen(
    title: String,
    subtitle: String?,
    hasAccount: Boolean,
    threads: List<ThreadEntity>,
    refreshing: Boolean,
    onRefresh: () -> Unit,
    onOpen: (String) -> Unit,
    onArchive: (String) -> Unit,
    onTrash: (String) -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        subtitle?.let {
                            Text(
                                it,
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                },
                actions = {
                    IconButton(onClick = onRefresh) {
                        Icon(Icons.Outlined.Refresh, contentDescription = "Rafraîchir")
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            when {
                !hasAccount -> Centered("Aucun compte email configuré", Icons.Outlined.Inbox)
                threads.isEmpty() && refreshing ->
                    CircularProgressIndicator(Modifier.align(Alignment.Center))
                else -> PullToRefreshBox(
                    isRefreshing = refreshing,
                    onRefresh = onRefresh,
                    modifier = Modifier.fillMaxSize(),
                ) {
                    if (threads.isEmpty()) {
                        Centered("Aucun message", Icons.Outlined.Inbox)
                    } else {
                        LazyColumn(Modifier.fillMaxSize()) {
                            items(threads, key = { it.id }) { thread ->
                                SwipeableThreadRow(
                                    thread = thread,
                                    onOpen = { onOpen(thread.id) },
                                    onArchive = { onArchive(thread.id) },
                                    onTrash = { onTrash(thread.id) },
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SwipeableThreadRow(
    thread: ThreadEntity,
    onOpen: () -> Unit,
    onArchive: () -> Unit,
    onTrash: () -> Unit,
) {
    // Swipe right = archive (green), left = trash (red) — the web's gestures.
    val dismissState = rememberSwipeToDismissBoxState(
        confirmValueChange = { value ->
            when (value) {
                SwipeToDismissBoxValue.StartToEnd -> { onArchive(); true }
                SwipeToDismissBoxValue.EndToStart -> { onTrash(); true }
                SwipeToDismissBoxValue.Settled -> false
            }
        },
    )
    SwipeToDismissBox(
        state = dismissState,
        backgroundContent = { SwipeBackground(dismissState.dismissDirection) },
    ) {
        ThreadRow(thread, onOpen)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SwipeBackground(direction: SwipeToDismissBoxValue) {
    val (color, icon, align) = when (direction) {
        SwipeToDismissBoxValue.StartToEnd ->
            Triple(Color(0xFF1E8E3E), Icons.Outlined.Archive, Alignment.CenterStart)
        SwipeToDismissBoxValue.EndToStart ->
            Triple(Color(0xFFD93025), Icons.Outlined.Delete, Alignment.CenterEnd)
        SwipeToDismissBoxValue.Settled ->
            Triple(MaterialTheme.colorScheme.surface, null, Alignment.Center)
    }
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(color)
            .padding(horizontal = 24.dp),
        contentAlignment = align,
    ) {
        icon?.let {
            Icon(it, contentDescription = null, tint = Color.White, modifier = Modifier.size(24.dp))
        }
    }
}

@Composable
private fun Centered(message: String, icon: ImageVector? = null) {
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
 * 40dp avatar, the correspondent and date on top, the subject with the star
 * below, then a snippet. Unread rows are bold; 64dp min height.
 */
@Composable
private fun ThreadRow(thread: ThreadEntity, onOpen: () -> Unit) {
    val unread = thread.unreadCount > 0
    val sender = (thread.senderName?.takeIf { it.isNotBlank() } ?: thread.senderEmail).orEmpty()
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surface)
            .heightIn(min = 64.dp)
            .clickable(onClick = onOpen)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Avatar(sender)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    sender.ifBlank { "(inconnu)" },
                    fontSize = 15.sp,
                    fontWeight = if (unread) FontWeight.SemiBold else FontWeight.Normal,
                    color = MaterialTheme.colorScheme.onSurface,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    formatDate(thread.orderKey),
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
    val color = AVATAR_COLORS[hashOf(label) % AVATAR_COLORS.size]
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
        val zoned = Instant.parse(iso).atZone(ZoneId.systemDefault())
        val now = Instant.now().atZone(ZoneId.systemDefault())
        if (zoned.toLocalDate() == now.toLocalDate()) TIME_FMT.format(zoned)
        else DAY_FMT.format(zoned)
    }.getOrDefault("")
}
