package me.sagiri.pulsebridge.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class StreamWatchdogTest {

    @Test
    fun disconnectedStreamNeverRecovers() {
        val watchdog = watchdog()

        assertFalse(watchdog.shouldRecover(100_000L))
    }

    @Test
    fun connectedStreamGetsGracePeriodThenRecovers() {
        val watchdog = watchdog()
        watchdog.onConnectionChanged(true, 1_000L)

        assertFalse(watchdog.shouldRecover(30_999L))
        assertTrue(watchdog.shouldRecover(31_000L))
    }

    @Test
    fun freshSamplesPostponeRecovery() {
        val watchdog = watchdog()
        watchdog.onConnectionChanged(true, 1_000L)
        watchdog.onSample(20_000L)

        assertFalse(watchdog.shouldRecover(49_999L))
        assertTrue(watchdog.shouldRecover(50_000L))
    }

    @Test
    fun recoveryIsRateLimitedUntilConnectionStateChanges() {
        val watchdog = watchdog()
        watchdog.onConnectionChanged(true, 1_000L)

        assertTrue(watchdog.shouldRecover(31_000L))
        assertFalse(watchdog.shouldRecover(60_999L))
        assertTrue(watchdog.shouldRecover(61_000L))

        watchdog.onConnectionChanged(false, 62_000L)
        assertFalse(watchdog.shouldRecover(100_000L))
    }

    private fun watchdog() = StreamWatchdog(
        stallAfterMs = 30_000L,
        recoveryCooldownMs = 30_000L,
    )
}
