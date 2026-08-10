package com.kubuno.mail

import android.net.Uri
import kotlinx.coroutines.flow.MutableStateFlow

/**
 * A UI-agnostic hand-off for "compose a message", raised when the system routes
 * a `mailto:` link or a share (ACTION_SEND) to this app — the intents that make
 * Android treat it as an email client.
 *
 * [MainActivity] parses the intent and pushes a [ComposeLaunch] here; the
 * Compose tree opens the composer pre-filled. Kept a plain object so the intent
 * handling does not depend on how the screens are wired.
 */
object ComposeBus {
    val request = MutableStateFlow<ComposeLaunch?>(null)

    fun open(launch: ComposeLaunch) { request.value = launch }
}

/** What a mailto/share fills into the composer. */
data class ComposeLaunch(
    val to: String = "",
    val cc: String = "",
    val bcc: String = "",
    val subject: String = "",
    val body: String = "",
    /** Attachments shared into the app, as content URIs to read on open. */
    val attachmentUris: List<Uri> = emptyList(),
)
