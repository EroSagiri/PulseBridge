package me.sagiri.pulsebridge.service

import android.app.job.JobInfo
import android.app.job.JobScheduler
import android.content.ComponentName
import android.content.Context
import me.sagiri.pulsebridge.PbLog

/**
 * A persisted system job is the last-resort path for OEMs that defer manifest
 * boot receivers under startup pressure. It also repairs an OEM-killed bridge
 * without waking the process more often than Android's minimum periodic rate.
 */
object StartupScheduler {
    private const val TAG = "PulseBridgeStartup"
    private const val JOB_ID = 0x505542
    private const val INTERVAL_MS = 15 * 60 * 1_000L
    private const val FLEX_MS = 5 * 60 * 1_000L

    fun sync(context: Context, enabled: Boolean) {
        val jobs = context.getSystemService(JobScheduler::class.java)
        if (!enabled) {
            jobs.cancel(JOB_ID)
            return
        }
        if (jobs.getPendingJob(JOB_ID) != null) return

        val job = JobInfo.Builder(
            JOB_ID,
            ComponentName(context, StartupJobService::class.java),
        )
            .setPersisted(true)
            .setPeriodic(INTERVAL_MS, FLEX_MS)
            .build()
        val result = jobs.schedule(job)
        PbLog.i(TAG, "startup_watchdog_scheduled", mapOf("result" to result))
    }
}
