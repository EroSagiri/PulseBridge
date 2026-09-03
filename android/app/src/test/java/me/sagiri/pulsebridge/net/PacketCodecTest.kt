package me.sagiri.pulsebridge.net

import org.junit.Assert.assertEquals
import org.junit.Test

class PacketCodecTest {
    @Test
    fun telemetryVectorMatchesRustWirePacket() {
        val packet = PacketCodec.encode(
            key = PacketCodec.parseKeyHex(
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            ),
            deviceId = 1,
            sessionId = 0x11223344,
            sequence = 1,
            timestampMs = 1_700_000_000_000,
            flags = 0x07,
            heartRate = 72,
            batteryPct = 85,
            restingHr = 51,
        )

        assertEquals(
            "504201010100000044332211010000000068e5cf8b010000" +
                "a5b72a2610bdfeb65f8eb2327d362c3982c0e553",
            packet.toHex(),
        )
    }

    private fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }
}
