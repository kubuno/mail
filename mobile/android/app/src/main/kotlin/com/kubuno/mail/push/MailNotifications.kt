package com.kubuno.mail.push

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import com.kubuno.mail.MainActivity
import com.kubuno.mail.R

/**
 * Builds the mail notification channels and posts new-message notifications.
 *
 * Stateless on purpose: the push receiver has no lifecycle, so everything is
 * derived from the [Context] handed in per call. The tap action is a deep link
 * into the thread (`kubuno-mail://thread/<id>`) resolved by [MainActivity], so a
 * notification always lands the user on the message that triggered it.
 */
object MailNotifications {

    /** Channel for "a new message arrived", the only push kind in v1. */
    const val CHANNEL_MAIL_RECEIVED = "mail_received"

    /** Deep-link scheme/host shared with the manifest intent-filter. */
    const val DEEP_LINK_SCHEME = "kubuno-mail"
    const val DEEP_LINK_HOST = "thread"

    /**
     * Creates the notification channels. Safe to call repeatedly (creating an
     * existing channel is a no-op) and a no-op below Android O.
     */
    fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val channel = NotificationChannel(
            CHANNEL_MAIL_RECEIVED,
            // French UI, matching the rest of the app.
            "Nouveaux messages",
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = "Notifications de nouveaux e-mails reçus"
        }
        manager.createNotificationChannel(channel)
    }

    /**
     * Posts a new-message notification. Missing/blank fields fall back to sane
     * defaults so a malformed push still yields a tappable notification rather
     * than nothing. Returns silently if the runtime notifications permission is
     * not granted (Android 13+), which the UI is responsible for requesting.
     */
    fun notifyMail(context: Context, payload: MailPushPayload) {
        ensureChannels(context)

        val title = payload.title?.takeIf { it.isNotBlank() } ?: "Nouveau message"
        val body = payload.body?.takeIf { it.isNotBlank() } ?: ""

        val builder = NotificationCompat.Builder(context, CHANNEL_MAIL_RECEIVED)
            .setSmallIcon(R.drawable.ic_kubuno_logo)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_EMAIL)

        payload.resourceId?.takeIf { it.isNotBlank() }?.let { threadId ->
            builder.setContentIntent(threadPendingIntent(context, threadId))
        }

        // A stable id per thread so repeated pushes update rather than stack.
        val notificationId = payload.resourceId?.hashCode() ?: title.hashCode()
        // NotificationManagerCompat guards the POST_NOTIFICATIONS permission; if
        // it is missing the system drops the post — no crash, no leak.
        NotificationManagerCompat.from(context).notify(notificationId, builder.build())
    }

    /** A PendingIntent that opens the thread reader through the deep-link path. */
    private fun threadPendingIntent(context: Context, threadId: String): PendingIntent {
        val uri = Uri.Builder()
            .scheme(DEEP_LINK_SCHEME)
            .authority(DEEP_LINK_HOST)
            .appendPath(threadId)
            .build()
        // Explicit component so the tap can never be captured by another app,
        // while the data URI still drives MainActivity's deep-link handling.
        val intent = Intent(Intent.ACTION_VIEW, uri, context, MainActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        }
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        // Distinct request code per thread so pending intents do not collide.
        return PendingIntent.getActivity(context, threadId.hashCode(), intent, flags)
    }
}
