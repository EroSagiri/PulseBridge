package me.sagiri.pulsebridge.service

import android.app.job.JobParameters
import android.app.job.JobService
import me.sagiri.pulsebridge.BridgeState
import me.sagiri.pulsebridge.PbLog
import me.sagiri.pulsebridge.Prefs

class StartupJobService : JobService() {
    override fun onStartJob(params: JobParameters): Boolean {
        val prefs = Prefs(this)
        if (!prefs.autoStart) {
            StartupScheduler.sync(this, enabled = false)
            return false
        }
        if (!prefs.isConfigured() || BridgeState.state.value.running) return false

        runCatching { BridgeService.startAfterBoot(this) }
            .onSuccess { PbLog.i(TAG, "startup_watchdog_recovery_requested") }
            .onFailure { PbLog.e(TAG, "startup_watchdog_recovery_failed", it) }
        return false
    }

    override fun onStopJob(params: JobParameters): Boolean = false

    companion object {
        private const val TAG = "PulseBridgeStartup"
    }
}
