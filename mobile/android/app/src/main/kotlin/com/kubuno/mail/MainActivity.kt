package com.kubuno.mail

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat
import com.kubuno.android.ui.theme.KubunoTheme
import com.kubuno.mail.push.MailNotifications
import com.kubuno.mail.ui.MailApp
import dagger.hilt.android.AndroidEntryPoint

// AppCompatActivity so per-app locales keep working down to minSdk, matching
// the drive app.
@AndroidEntryPoint
class MainActivity : AppCompatActivity() {

    // Result is fire-and-forget: notifications simply won't post if denied, and
    // the app keeps working. No UI depends on the outcome.
    private val requestNotifications =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { /* ignored */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // The launch intent may be a notification deep link; feed the bus before
        // the tree composes so the reader can open straight away.
        handleDeepLink(intent)
        maybeRequestNotificationPermission()
        setContent {
            KubunoTheme {
                MailApp()
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // Keep getIntent() consistent for anything that reads it later.
        setIntent(intent)
        handleDeepLink(intent)
    }

    /**
     * Extracts a `kubuno-mail://thread/<id>` deep link and forwards the thread
     * id to [DeepLinkBus]. The Compose layer decides what to do with it, keeping
     * the Activity free of navigation knowledge.
     */
    private fun handleDeepLink(intent: Intent?) {
        val uri: Uri = intent?.data ?: return
        if (uri.scheme == MailNotifications.DEEP_LINK_SCHEME &&
            uri.host == MailNotifications.DEEP_LINK_HOST
        ) {
            // Path is "/<threadId>"; lastPathSegment yields the id.
            DeepLinkBus.request(uri.lastPathSegment)
        }
    }

    /** Ask for POST_NOTIFICATIONS on Android 13+ if not already granted. */
    private fun maybeRequestNotificationPermission() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
        val granted = ContextCompat.checkSelfPermission(
            this, Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED
        if (!granted) {
            requestNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}
