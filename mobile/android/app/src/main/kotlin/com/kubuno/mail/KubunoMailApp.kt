package com.kubuno.mail

import android.app.Application
import coil3.ImageLoader
import coil3.PlatformContext
import coil3.SingletonImageLoader
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import com.kubuno.android.account.AccountCallFactory
import com.kubuno.android.account.AccountManagerBridge
import com.kubuno.mail.push.MailPush
import dagger.hilt.android.HiltAndroidApp
import javax.inject.Inject

@HiltAndroidApp
class KubunoMailApp : Application(), SingletonImageLoader.Factory {

    @Inject lateinit var accountBridge: AccountManagerBridge
    @Inject lateinit var callFactory: AccountCallFactory

    override fun onCreate() {
        super.onCreate()
        // Keep the system accounts in step with ours, exactly like the drive
        // app: whichever app the user opens keeps the shared list current.
        accountBridge.install()
        // Try to (re)attach to a UnifiedPush distributor. No-op — never a crash —
        // when none is installed, so the app stays fully usable without push.
        MailPush.ensureRegistered(this)
    }

    /** Avatars ride each account's authenticated client, chosen per URL. */
    override fun newImageLoader(context: PlatformContext): ImageLoader =
        ImageLoader.Builder(context)
            .components {
                add(OkHttpNetworkFetcherFactory(callFactory = { callFactory }))
            }
            .build()
}
