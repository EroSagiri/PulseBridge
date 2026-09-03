package me.sagiri.pulsebridge

/**
 * In-memory evidence for one foreground-service run. This is deliberately
 * local to Android: it is for diagnosing a real device, not part of telemetry
 * or the shared WebSocket contract.
 */
data class DiagnosticState(
    val lastSourceEvent: String? = null,
    val lastSourceEventAtMs: Long = 0L,
    val watchdogRecoveries: Int = 0,
    val lastWatchdogReason: String? = null,
    val lastUdpSentAtMs: Long = 0L,
    val udpSendFailures: Long = 0L,
    val lastUdpError: String? = null,
    val lastUdpErrorAtMs: Long = 0L,
) {
    fun sourceEvent(event: String, nowMs: Long): DiagnosticState = copy(
        lastSourceEvent = event,
        lastSourceEventAtMs = nowMs,
    )

    fun watchdogRecovery(reason: String, nowMs: Long): DiagnosticState = copy(
        lastSourceEvent = "watchdog: $reason",
        lastSourceEventAtMs = nowMs,
        watchdogRecoveries = watchdogRecoveries + 1,
        lastWatchdogReason = reason,
    )

    fun udpSent(nowMs: Long): DiagnosticState = copy(lastUdpSentAtMs = nowMs)

    fun udpFailure(error: String, nowMs: Long): DiagnosticState = copy(
        udpSendFailures = udpSendFailures + 1,
        lastUdpError = error,
        lastUdpErrorAtMs = nowMs,
    )
}

fun ageMs(nowMs: Long, timestampMs: Long): Long? =
    if (timestampMs == 0L) null else (nowMs - timestampMs).coerceAtLeast(0L)
