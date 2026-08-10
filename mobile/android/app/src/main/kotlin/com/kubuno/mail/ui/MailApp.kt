package com.kubuno.mail.ui

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.provider.Settings
import android.util.Log
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Send
import androidx.compose.material.icons.outlined.Drafts
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.StarBorder
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import com.kubuno.android.ui.shell.KubunoTopBar
import com.kubuno.android.ui.shell.KubunoAccountPanel
import com.kubuno.android.ui.shell.UiAccount
import com.kubuno.mail.R
import com.kubuno.mail.data.MailFolder
import com.kubuno.mail.data.MailRepository
import com.kubuno.mail.data.ThreadEntity
import com.kubuno.mail.data.key
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import javax.inject.Inject

/**
 * Offline-first inbox for the active shared account, driven by the /changes
 * delta. Switching accounts (from the shared account panel) re-keys the list;
 * switching folders (bottom bar or drawer) re-queries the cache.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@HiltViewModel
class InboxViewModel @Inject constructor(
    sharedAccounts: SharedAccounts,
    private val repo: MailRepository,
) : ViewModel() {

    val accounts: List<SharedAccount> = sharedAccounts.list()

    private val _active = MutableStateFlow(accounts.firstOrNull())
    val active: StateFlow<SharedAccount?> = _active

    private val _folder = MutableStateFlow(MailFolder.INBOX)
    val folder: StateFlow<MailFolder> = _folder

    val threads: StateFlow<List<ThreadEntity>> =
        combine(_active, _folder) { account, folder -> account to folder }
            .flatMapLatest { (account, folder) ->
                account?.let { repo.threads(it, folder) } ?: flowOf(emptyList())
            }
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyList())

    private val _refreshing = MutableStateFlow(false)
    val refreshing: StateFlow<Boolean> = _refreshing

    init {
        if (_active.value != null) refresh()
    }

    fun switchAccount(key: String) {
        val account = accounts.firstOrNull { it.key == key } ?: return
        if (_active.value?.key == account.key) return
        _active.value = account
        _folder.value = MailFolder.INBOX
        refresh()
    }

    fun selectFolder(folder: MailFolder) {
        if (_folder.value == folder) return
        _folder.value = folder
        refresh()
    }

    fun refresh() {
        val account = _active.value ?: return
        val folder = _folder.value
        viewModelScope.launch {
            _refreshing.value = true
            runCatching { repo.refresh(account, folder) }
            _refreshing.value = false
        }
    }

    fun archive(id: String) = _active.value?.let { a ->
        viewModelScope.launch { repo.archive(a, id) }
    }

    fun trash(id: String) = _active.value?.let { a ->
        viewModelScope.launch { repo.trash(a, id) }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MailApp(
    viewModel: InboxViewModel = hiltViewModel(),
    reader: ReaderViewModel = hiltViewModel(),
) {
    val context = LocalContext.current
    val threads by viewModel.threads.collectAsStateWithLifecycle()
    val refreshing by viewModel.refreshing.collectAsStateWithLifecycle()
    val folder by viewModel.folder.collectAsStateWithLifecycle()
    val active by viewModel.active.collectAsStateWithLifecycle()

    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    var panelOpen by remember { mutableStateOf(false) }
    var openThread by remember { mutableStateOf<String?>(null) }
    var compose by remember { mutableStateOf<ComposePrefill?>(null) }

    ModalNavigationDrawer(
        drawerState = drawerState,
        drawerContent = {
            MailDrawer(current = folder) { selected ->
                scope.launch { drawerState.close() }
                viewModel.selectFolder(selected)
            }
        },
    ) {
        Scaffold(
            topBar = {
                KubunoTopBar(
                    avatarLabel = active?.label,
                    onAvatarClick = { panelOpen = true },
                    onOpenMenu = { scope.launch { drawerState.open() } },
                    onSearch = { /* search screen lands in M4 */ },
                    onNotifications = { /* notifications land with push in M5 */ },
                    onSettings = { /* settings land in M3 */ },
                )
            },
            bottomBar = { MailBottomBar(folder, viewModel::selectFolder) },
            floatingActionButton = {
                FloatingActionButton(onClick = { compose = ComposePrefill() }) {
                    Icon(Icons.Outlined.Edit, contentDescription = "Nouveau message")
                }
            },
        ) { padding ->
            InboxBody(
                hasAccount = active != null,
                threads = threads,
                refreshing = refreshing,
                onRefresh = viewModel::refresh,
                onOpen = { id -> openThread = id; reader.open(id) },
                onArchive = viewModel::archive,
                onTrash = viewModel::trash,
                modifier = Modifier.padding(padding),
            )
        }
    }

    if (panelOpen) {
        KubunoAccountPanel(
            email = active?.email,
            displayName = active?.label,
            avatarUrl = null,
            accounts = viewModel.accounts.map {
                UiAccount(id = it.key, label = it.label, host = it.host)
            },
            activeId = active?.key,
            onSwitchAccount = { panelOpen = false; viewModel.switchAccount(it) },
            onManageDeviceAccounts = { panelOpen = false; openDeviceAccounts(context) },
            onDismiss = { panelOpen = false },
        )
    }

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
            onReply = { prefill -> compose = prefill },
        )
    }

    compose?.let { prefill ->
        BackHandler { compose = null }
        ComposeScreen(prefill = prefill, onClose = { compose = null })
    }
}

@Composable
private fun MailDrawer(current: MailFolder, onSelect: (MailFolder) -> Unit) {
    ModalDrawerSheet {
        Row(Modifier.fillMaxWidth().padding(24.dp)) {
            Icon(
                painter = painterResource(com.kubuno.android.ui.R.drawable.ic_kubuno_logo),
                contentDescription = null,
                tint = MaterialTheme.colorScheme.primary,
                modifier = Modifier.size(width = 22.dp, height = 24.dp),
            )
            Text(
                "  Kubuno Mail",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }
        MailFolder.entries.forEach { folder ->
            NavigationDrawerItem(
                icon = { Icon(drawerIcon(folder), contentDescription = null) },
                label = { Text(folder.label) },
                selected = folder == current,
                onClick = { onSelect(folder) },
                modifier = Modifier.padding(horizontal = 12.dp),
            )
        }
    }
}

private fun drawerIcon(folder: MailFolder): ImageVector = when (folder) {
    MailFolder.INBOX -> Icons.Outlined.Inbox
    MailFolder.STARRED -> Icons.Outlined.StarBorder
    MailFolder.SENT -> Icons.AutoMirrored.Outlined.Send
    MailFolder.DRAFTS -> Icons.Outlined.Drafts
}

/** The system's account settings — where accounts are added and removed. */
private fun openDeviceAccounts(context: Context) {
    try {
        context.startActivity(
            Intent(Settings.ACTION_SYNC_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        )
    } catch (e: ActivityNotFoundException) {
        Log.w("MailApp", "No account settings activity on this device", e)
    }
}
