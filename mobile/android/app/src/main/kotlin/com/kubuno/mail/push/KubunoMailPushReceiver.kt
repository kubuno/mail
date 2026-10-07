package com.kubuno.mail.push

import android.content.Context
import android.util.Log
import com.kubuno.android.account.BrokeredClients
import com.kubuno.android.account.SharedAccount
import com.kubuno.android.account.SharedAccounts
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.MessagingReceiver
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage
import java.util.Locale
import javax.inject.Inject

/**
 * Bridges UnifiedPush distributor events to the core and to notifications.
 *
 * Hilt supports field injection on a BroadcastReceiver via [AndroidEntryPoint]:
 * the generated wrapper injects the fields before dispatching, so
 * [SharedAccounts] and [BrokeredClients] are ready by the time a callback runs.
 *
 * Network calls (register/unregister with the core) run off the main thread on
 * an app-scoped coroutine, and [goAsync] keeps the receiver alive until they
 * finish — a BroadcastReceiver is otherwise torn down as soon as the callback
 * returns.
 */
@AndroidEntryPoint
class KubunoMailPushReceiver : MessagingReceiver() {

    @Inject lateinit var sharedAccounts: SharedAccounts
    @Inject lateinit var brokered: BrokeredClients

    // Not tied to any UI lifecycle: push events arrive with the app in any state.
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    override fun onNewEndpoint(context: Context, endpoint: PushEndpoint, instance: String) {
        // v1 binds the endpoint to the active (first) shared account, matching
        // the inbox's own notion of "active account".
        val account = sharedAccounts.list().firstOrNull()
        if (account == null) {
            Log.w(TAG, "New push endpoint but no shared account to bind it to")
            return
        }
        val pending = goAsync()
        scope.launch {
            try {
                registerDevice(context, account, endpoint.url)
            } catch (e: Exception) {
                Log.e(TAG, "Failed to register push endpoint with the core", e)
            } finally {
                pending.finish()
            }
        }
    }

    override fun onMessage(context: Context, message: PushMessage, instance: String) {
        // content is the (already decrypted, when applicable) push body.
        val raw = try {
            String(message.content, Charsets.UTF_8)
        } catch (e: Exception) {
            Log.w(TAG, "Push message content was not valid UTF-8", e)
            null
        }
        val payload = MailPushPayload.parse(raw)
        MailNotifications.notifyMail(context, payload)
    }

    override fun onRegistrationFailed(context: Context, reason: FailedReason, instance: String) {
        // Nothing to do but record it; a retry happens on the next app start.
        Log.w(TAG, "UnifiedPush registration failed: $reason")
    }

    override fun onUnregistered(context: Context, instance: String) {
        Log.i(TAG, "UnifiedPush unregistered for instance=$instance")
        val account = sharedAccounts.list().firstOrNull() ?: run {
            PushPrefs.clear(context)
            return
        }
        val pending = goAsync()
        scope.launch {
            try {
                unregisterDevice(context, account)
            } catch (e: Exception) {
                Log.e(TAG, "Failed to unregister push device from the core", e)
            } finally {
                PushPrefs.clear(context)
                pending.finish()
            }
        }
    }

    /**
     * POST /api/v1/me/push/devices — body confirmed against the core handler
     * (crates/kubuno-core/src/handlers/push.rs, RegisterDeviceDto):
     * `{ provider, device_token, app_id?, locale? }`, response `{ "id": <uuid> }`.
     * The brokered client already carries the account's borrowed access token.
     */
    private fun registerDevice(context: Context, account: SharedAccount, endpoint: String) {
        val client = brokered.of(account)
        val json = JSONObject()
            .put("provider", "unifiedpush")
            .put("device_token", endpoint)
            .put("app_id", context.packageName) // com.kubuno.mail.android
            .put("locale", Locale.getDefault().toLanguageTag())
            .toString()

        val request = Request.Builder()
            .url(client.serverUrl + PUSH_DEVICES_PATH)
            .post(json.toRequestBody(JSON_MEDIA_TYPE))
            .build()

        client.okHttpClient.newCall(request).execute().use { response ->
            if (!response.isSuccessful) {
                Log.e(TAG, "register_device HTTP ${response.code}")
                return
            }
            val id = response.body?.string()?.let {
                runCatching { JSONObject(it).optString("id") }.getOrNull()
            }?.takeIf { it.isNotBlank() }
            if (id != null) {
                // Persist so we can DELETE this exact registration later.
                PushPrefs.save(context, endpoint = endpoint, registrationId = id, account = account.systemName)
                Log.i(TAG, "Push device registered (id=$id)")
            } else {
                Log.w(TAG, "register_device returned no id")
            }
        }
    }

    /** DELETE /api/v1/me/push/devices/{id} — best-effort, using the stored id. */
    private fun unregisterDevice(context: Context, account: SharedAccount) {
        val id = PushPrefs.registrationId(context) ?: return
        val client = brokered.of(account)
        val request = Request.Builder()
            .url(client.serverUrl + PUSH_DEVICES_PATH + "/" + id)
            .delete()
            .build()
        client.okHttpClient.newCall(request).execute().use { response ->
            if (!response.isSuccessful && response.code != 404) {
                Log.e(TAG, "delete_device HTTP ${response.code}")
            }
        }
    }

    private companion object {
        const val TAG = "MailPush"
        const val PUSH_DEVICES_PATH = "/api/v1/me/push/devices"
        val JSON_MEDIA_TYPE = "application/json".toMediaType()
    }
}
