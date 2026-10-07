package com.kubuno.mail

import kotlinx.coroutines.flow.MutableStateFlow

/**
 * A process-wide, UI-agnostic hand-off for an incoming deep link.
 *
 * [MainActivity] parses `kubuno-mail://thread/<id>` from the launch (or new)
 * intent and pushes the thread id here; the Compose tree observes [target] and
 * opens the reader. Keeping it a plain object (not a Hilt binding) decouples the
 * Activity's intent handling from the screen wiring, so the notification tap
 * path does not depend on the navigation graph being assembled a certain way.
 *
 * The value is a one-shot request: whoever consumes it must reset it to `null`
 * so the same thread does not re-open on recomposition or configuration change.
 */
object DeepLinkBus {
    /** The thread id requested by the latest deep link, or null when consumed. */
    val target = MutableStateFlow<String?>(null)

    /** Records a deep-link request. Ignores blank ids defensively. */
    fun request(threadId: String?) {
        if (!threadId.isNullOrBlank()) {
            target.value = threadId
        }
    }
}
