package me.sagiri.pulsebridge.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import me.sagiri.pulsebridge.PbLog
import me.sagiri.pulsebridge.BridgeState
import me.sagiri.pulsebridge.Prefs

/** Brings the bridge back after reboot/update, but only if the user opted in. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action !in SUPPORTED_ACTIONS) return
        val prefs = Prefs(context)
        val shouldStart = prefs.autoStart && prefs.isConfigured()
        PbLog.i(TAG, "boot_event", mapOf("action" to intent.action, "start" to shouldStart))
        StartupScheduler.sync(context, prefs.autoStart)
        if (shouldStart && !BridgeState.state.value.running) {
            runCatching { BridgeService.startAfterBoot(context) }
                .onFailure {
                    PbLog.e(TAG, "boot_recovery_failed", it, mapOf("action" to intent.action))
                }
        }
    }

    companion object {
        private const val TAG = "PulseBridgeBoot"
        private const val ACTION_OPLUS_BOOT_COMPLETED = "oplus.intent.action.BOOT_COMPLETED"
        private const val ACTION_OPPO_BOOT_COMPLETED = "oppo.intent.action.BOOT_COMPLETED"
        private val SUPPORTED_ACTIONS = setOf(
            Intent.ACTION_LOCKED_BOOT_COMPLETED,
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED,
            Intent.ACTION_USER_UNLOCKED,
            Intent.ACTION_USER_PRESENT,
            ACTION_OPLUS_BOOT_COMPLETED,
            ACTION_OPPO_BOOT_COMPLETED,
        )
    }
}
