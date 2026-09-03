package me.sagiri.pulsebridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class DiagnosticStateTest {

    @Test
    fun sourceEventsKeepEventAndTimestamp() {
        val state = DiagnosticState().sourceEvent("link down, gatt status=133", 1_000L)

        assertEquals("link down, gatt status=133", state.lastSourceEvent)
        assertEquals(1_000L, state.lastSourceEventAtMs)
    }

    @Test
    fun watchdogRecoveryIsCountedAndVisible() {
        val state = DiagnosticState()
            .watchdogRecovery("no heart-rate notification for 30s", 2_000L)
            .watchdogRecovery("no heart-rate notification for 30s", 3_000L)

        assertEquals(2, state.watchdogRecoveries)
        assertEquals("no heart-rate notification for 30s", state.lastWatchdogReason)
        assertEquals("watchdog: no heart-rate notification for 30s", state.lastSourceEvent)
        assertEquals(3_000L, state.lastSourceEventAtMs)
    }

    @Test
    fun udpFailuresRemainAfterARecovery() {
        val state = DiagnosticState()
            .udpFailure("send failed: network unreachable", 1_000L)
            .udpSent(2_000L)

        assertEquals(1L, state.udpSendFailures)
        assertEquals("send failed: network unreachable", state.lastUdpError)
        assertEquals(1_000L, state.lastUdpErrorAtMs)
        assertEquals(2_000L, state.lastUdpSentAtMs)
    }

    @Test
    fun ageUsesMonotonicDirectionAndUnknownIsNull() {
        assertEquals(1_500L, ageMs(2_500L, 1_000L))
        assertEquals(0L, ageMs(900L, 1_000L))
        assertNull(ageMs(2_500L, 0L))
    }
}
