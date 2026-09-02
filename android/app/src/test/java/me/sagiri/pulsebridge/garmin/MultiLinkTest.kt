package me.sagiri.pulsebridge.garmin

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Pins the frame formats to the bytes actually captured off a Forerunner 255.
 * Every literal here is copied from docs/phase0-multilink.md -- if one of these
 * fails, either the parser drifted or the watch changed, and both are worth
 * stopping for.
 */
class MultiLinkTest {

    private fun bytes(hex: String): ByteArray =
        hex.trim().split(" ").map { it.toInt(16).toByte() }.toByteArray()

    @Test
    fun registrationFrameMatchesTheOneTheWatchAccepted() {
        // 00 00 | 50 42 00 00 00 00 00 00 | 06 00 | 00
        assertEquals(
            "00 00 50 42 00 00 00 00 00 00 06 00 00",
            MultiLink.registrationFrame(MultiLink.SVC_REAL_TIME_HR)
                .joinToString(" ") { "%02x".format(it) },
        )
    }

    @Test
    fun parsesTheRealTimeHrRegistrationReply() {
        val reply = MultiLink.parseRegistrationReply(
            bytes("00 01 50 42 00 00 00 00 00 00 06 00 00 24 00 00")
        )!!
        assertEquals(MultiLink.SVC_REAL_TIME_HR, reply.serviceId)
        assertEquals(MultiLink.STATUS_SUCCESS, reply.status)
        assertEquals(0x24, reply.handle)
    }

    @Test
    fun handlesAreNotConstants() {
        // Same capture, different registration order: REGISTRATION got 0x23 and
        // GFDI got 0x25 purely by being registered first and last.
        assertEquals(
            0x23,
            MultiLink.parseRegistrationReply(
                bytes("00 01 50 42 00 00 00 00 00 00 04 00 00 23 00 02")
            )!!.handle,
        )
        assertEquals(
            0x25,
            MultiLink.parseRegistrationReply(
                bytes("00 01 50 42 00 00 00 00 00 00 01 00 00 25 00 00")
            )!!.handle,
        )
    }

    @Test
    fun dataFramesAreNotMistakenForRegistrationReplies() {
        assertNull(MultiLink.parseRegistrationReply(bytes("24 03 42 33 ff ff")))
    }

    @Test
    fun parsesCapturedHeartRateFrames() {
        // 0x42..0x40 is 66, 65, 64 bpm; 0x33 is the resting rate of 51 that was
        // cross-checked against Garmin Connect for the same day.
        val captured = listOf(
            "24 03 42 33 ff ff" to 66,
            "24 03 41 33 ff ff" to 65,
            "24 03 40 33 ff ff" to 64,
            "24 03 43 33 ff ff" to 67,
        )
        for ((hex, expected) in captured) {
            val frame = MultiLink.parseHrFrame(0x24, bytes(hex))!!
            assertEquals(expected, frame.heartRate)
            assertEquals(51, frame.restingHr)
        }
    }

    @Test
    fun ignoresFramesForAnotherHandle() {
        assertNull(MultiLink.parseHrFrame(0x24, bytes("25 03 42 33 ff ff")))
    }

    @Test
    fun rejectsUnusableHeartRates() {
        assertNull(MultiLink.parseHrFrame(0x24, bytes("24 03 00 33 ff ff")))
        assertNull(MultiLink.parseHrFrame(0x24, bytes("24 03 ff 33 ff ff")))
        assertNull(MultiLink.parseHrFrame(0x24, bytes("24 03")))
    }

    @Test
    fun missingRestingRateReadsAsUnknownRatherThanZero() {
        assertNull(MultiLink.parseHrFrame(0x24, bytes("24 03 42 00 ff ff"))!!.restingHr)
        assertNull(MultiLink.parseHrFrame(0x24, bytes("24 03 42 ff ff ff"))!!.restingHr)
    }

    @Test
    fun garminUuidsMatchTheDeviceTable() {
        assertEquals("6a4e2800-667b-11e3-949a-0800200c9a66", MultiLink.SERVICE.toString())
        assertEquals("6a4e2810-667b-11e3-949a-0800200c9a66", MultiLink.garmin(0x2810).toString())
        assertEquals("6a4e2840-667b-11e3-949a-0800200c9a66", MultiLink.garmin(0x2840).toString())
    }
}
