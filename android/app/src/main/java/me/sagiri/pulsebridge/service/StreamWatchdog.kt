package me.sagiri.pulsebridge.service

/**
 * Detects the Android BLE failure mode where GATT still reports CONNECTED but
 * notifications have silently stopped. All times are monotonic elapsed time.
 */
internal class StreamWatchdog(
    private val stallAfterMs: Long,
    private val recoveryCooldownMs: Long,
) {
    private var connected = false
    private var connectedAtMs = 0L
    private var lastSampleAtMs = 0L
    private var lastRecoveryAtMs = 0L

    fun onConnectionChanged(isConnected: Boolean, nowMs: Long) {
        connected = isConnected
        if (isConnected) {
            connectedAtMs = nowMs
        } else {
            connectedAtMs = 0L
            lastSampleAtMs = 0L
        }
    }

    fun onSample(nowMs: Long) {
        lastSampleAtMs = nowMs
    }

    fun shouldRecover(nowMs: Long): Boolean {
        if (!connected) return false
        val lastProgressAtMs = maxOf(connectedAtMs, lastSampleAtMs)
        if (lastProgressAtMs == 0L || nowMs - lastProgressAtMs < stallAfterMs) return false
        if (lastRecoveryAtMs != 0L && nowMs - lastRecoveryAtMs < recoveryCooldownMs) return false
        lastRecoveryAtMs = nowMs
        return true
    }
}
