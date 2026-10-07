package com.kubuno.mail.push

import android.content.Context

/**
 * Small persistent record of the current push registration, so the app can
 * delete exactly the device it registered when the distributor unregisters.
 *
 * Stored in the private SharedPreferences file `kubuno-mail-push`. This holds no
 * secret: the endpoint URL and the core's registration id are not credentials.
 */
object PushPrefs {
    private const val FILE = "kubuno-mail-push"
    private const val KEY_ENDPOINT = "endpoint"
    private const val KEY_REGISTRATION_ID = "registration_id"
    private const val KEY_ACCOUNT = "account_system_name"

    private fun prefs(context: Context) =
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    fun save(context: Context, endpoint: String, registrationId: String, account: String) {
        prefs(context).edit()
            .putString(KEY_ENDPOINT, endpoint)
            .putString(KEY_REGISTRATION_ID, registrationId)
            .putString(KEY_ACCOUNT, account)
            .apply()
    }

    fun registrationId(context: Context): String? =
        prefs(context).getString(KEY_REGISTRATION_ID, null)

    fun clear(context: Context) {
        prefs(context).edit().clear().apply()
    }
}
