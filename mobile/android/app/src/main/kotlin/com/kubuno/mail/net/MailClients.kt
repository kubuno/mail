package com.kubuno.mail.net

import com.kubuno.android.account.BrokeredClients
import com.kubuno.android.account.SharedAccount
import dagger.hilt.android.qualifiers.ApplicationContext
import android.content.Context
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.serialization.json.Json
import okhttp3.MediaType.Companion.toMediaType
import retrofit2.Retrofit
import retrofit2.converter.kotlinx.serialization.asConverterFactory

/**
 * Builds a [MailApi] per shared account, over the account's borrowed-token
 * client. The mail app is a consumer: it never holds a refresh token, it talks
 * to each instance through the client :core-account brokers for it.
 */
@Singleton
class MailClients @Inject constructor(
    @ApplicationContext context: Context,
    private val brokered: BrokeredClients,
) {
    private val json = Json {
        ignoreUnknownKeys = true
        explicitNulls = false
    }
    private val apis = ConcurrentHashMap<String, MailApi>()

    fun api(account: SharedAccount): MailApi = apis.computeIfAbsent(account.systemName) {
        val client = brokered.of(account)
        Retrofit.Builder()
            .baseUrl(client.serverUrl + "/")
            .client(client.okHttpClient)
            .addConverterFactory(json.asConverterFactory("application/json".toMediaType()))
            .build()
            .create(MailApi::class.java)
    }
}
