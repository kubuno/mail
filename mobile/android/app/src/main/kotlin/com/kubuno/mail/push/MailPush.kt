package com.kubuno.mail.push

import android.content.Context
import android.util.Log
import org.unifiedpush.android.connector.UnifiedPush

/**
 * Entry point for wiring the app to a UnifiedPush distributor at startup.
 *
 * Design goal: the app stays perfectly usable with *no* distributor installed.
 * We never open a chooser dialog or start an activity from here — that requires
 * an Activity context and a user decision, which is out of scope for the v1
 * auto-register path. We only (re)register when a distributor is unambiguous:
 * one already saved, or exactly one installed.
 */
object MailPush {
    private const val TAG = "MailPush"

    /**
     * Best-effort registration. Called from [android.app.Application.onCreate]
     * with the application context; wrapped so a missing/misbehaving distributor
     * can never crash app startup.
     */
    fun ensureRegistered(context: Context) {
        // Channels exist regardless of push state, so a later notification path
        // (or a foreground poll) always has a channel to post to.
        MailNotifications.ensureChannels(context)

        try {
            val saved = UnifiedPush.getSavedDistributor(context)
            if (saved != null) {
                // Refresh the existing registration; if the distributor was
                // uninstalled, register() resolves to a no-op internally.
                UnifiedPush.register(context)
                return
            }
            val distributors = UnifiedPush.getDistributors(context)
            when {
                distributors.isEmpty() ->
                    Log.i(TAG, "No UnifiedPush distributor installed; push disabled")
                distributors.size == 1 -> {
                    // Unambiguous choice: adopt it without prompting.
                    UnifiedPush.saveDistributor(context, distributors.first())
                    UnifiedPush.register(context)
                }
                else ->
                    // A choice is needed; a settings-screen action can drive
                    // UnifiedPush.tryUseDefaultDistributor from an Activity later.
                    Log.i(TAG, "Multiple UnifiedPush distributors; awaiting user selection")
            }
        } catch (e: Exception) {
            Log.e(TAG, "UnifiedPush registration skipped", e)
        }
    }
}
