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
        handleIntent(intent)
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
        handleIntent(intent)
    }

    /**
     * Routes the launch intent: a notification deep link opens a thread; a
     * `mailto:` link or a share (ACTION_SEND) opens the composer — the intents
     * that let the system treat this as an email client. The Compose layer acts
     * on the buses, keeping the Activity free of screen knowledge.
     */
    private fun handleIntent(intent: Intent?) {
        intent ?: return
        val data = intent.data
        when {
            data?.scheme == MailNotifications.DEEP_LINK_SCHEME &&
                data.host == MailNotifications.DEEP_LINK_HOST ->
                DeepLinkBus.request(data.lastPathSegment)

            data?.scheme == "mailto" -> ComposeBus.open(mailtoLaunch(data, intent))

            intent.action == Intent.ACTION_SEND || intent.action == Intent.ACTION_SEND_MULTIPLE ->
                ComposeBus.open(shareLaunch(intent))
        }
    }

    /** Turns a mailto: URI (to/cc/subject/body) into a compose request. */
    private fun mailtoLaunch(uri: Uri, intent: Intent): ComposeLaunch {
        val mailTo = runCatching { android.net.MailTo.parse(uri.toString()) }.getOrNull()
        return ComposeLaunch(
            to = mailTo?.to.orEmpty(),
            cc = mailTo?.cc.orEmpty(),
            subject = mailTo?.subject ?: intent.getStringExtra(Intent.EXTRA_SUBJECT).orEmpty(),
            body = mailTo?.body ?: intent.getStringExtra(Intent.EXTRA_TEXT).orEmpty(),
        )
    }

    /** Turns a share (with recipients/subject/text and attachments) into a request. */
    private fun shareLaunch(intent: Intent): ComposeLaunch = ComposeLaunch(
        to = intent.getStringArrayExtra(Intent.EXTRA_EMAIL)?.joinToString(", ").orEmpty(),
        cc = intent.getStringArrayExtra(Intent.EXTRA_CC)?.joinToString(", ").orEmpty(),
        bcc = intent.getStringArrayExtra(Intent.EXTRA_BCC)?.joinToString(", ").orEmpty(),
        subject = intent.getStringExtra(Intent.EXTRA_SUBJECT).orEmpty(),
        body = intent.getStringExtra(Intent.EXTRA_TEXT).orEmpty(),
        attachmentUris = streamUris(intent),
    )

    private fun streamUris(intent: Intent): List<Uri> = when (intent.action) {
        Intent.ACTION_SEND_MULTIPLE ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
            }
        else -> {
            val uri = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
            }
            listOfNotNull(uri)
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
