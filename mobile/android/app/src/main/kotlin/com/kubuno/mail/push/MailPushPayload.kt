package com.kubuno.mail.push

import android.util.Log
import org.json.JSONObject

/**
 * The decoded push payload, as forwarded by the core from the mail module's
 * `mail.received` event: `{event_type|type, module_id, title, body, resource_id}`.
 *
 * Parsed with [org.json] rather than kotlinx.serialization so a malformed or
 * unexpected message can never throw into the receiver — every field is
 * optional and read defensively.
 */
data class MailPushPayload(
    val eventType: String?,
    val moduleId: String?,
    val title: String?,
    val body: String?,
    /** The thread id the notification should deep-link to, if any. */
    val resourceId: String?,
) {
    companion object {
        private const val TAG = "MailPush"

        /** Never throws: returns an empty payload when the JSON is unusable. */
        fun parse(raw: String?): MailPushPayload {
            if (raw.isNullOrBlank()) return empty()
            val obj = try {
                JSONObject(raw)
            } catch (e: Exception) {
                Log.w(TAG, "Unparseable push payload, showing a generic notification", e)
                return empty()
            }
            return MailPushPayload(
                // The core may emit either key depending on the event shape.
                eventType = obj.optStringOrNull("event_type") ?: obj.optStringOrNull("type"),
                moduleId = obj.optStringOrNull("module_id"),
                title = obj.optStringOrNull("title"),
                body = obj.optStringOrNull("body"),
                resourceId = obj.optStringOrNull("resource_id"),
            )
        }

        private fun empty() = MailPushPayload(null, null, null, null, null)

        /** [JSONObject.optString] returns "" for absent keys; normalize to null. */
        private fun JSONObject.optStringOrNull(key: String): String? =
            if (has(key) && !isNull(key)) optString(key).takeIf { it.isNotEmpty() } else null
    }
}
