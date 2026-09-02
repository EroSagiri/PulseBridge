package me.sagiri.pulsebridge.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import me.sagiri.pulsebridge.Prefs

/** Brings the bridge back after a reboot, but only if the user opted in. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        val prefs = Prefs(context)
        if (prefs.autoStart && prefs.isConfigured()) {
            BridgeService.start(context)
        }
    }
}
