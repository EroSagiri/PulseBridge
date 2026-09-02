package me.sagiri.pulsebridge.net

import java.nio.ByteBuffer
import java.nio.ByteOrder
import javax.crypto.Cipher
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Byte-compatible with `server/src/protocol.rs`. Any change here needs the same
 * change there and a bump of [VERSION]; the spec lives in protocol/protocol.md.
 */
object PacketCodec {

    const val MAGIC = 0x5042
    const val VERSION: Byte = 1
    const val TYPE_TELEMETRY: Byte = 1
    const val HEADER_LEN = 24
    const val PACKET_LEN = 44

    const val FLAG_HR_VALID = 0x01
    const val FLAG_CONTACT_OK = 0x02
    const val FLAG_WATCH_CONNECTED = 0x04
    const val FLAG_HEARTBEAT = 0x08

    fun parseKeyHex(hex: String): ByteArray {
        require(hex.length == 64) { "key must be 64 hex characters" }
        return ByteArray(32) { i ->
            hex.substring(i * 2, i * 2 + 2).toInt(16).toByte()
        }
    }

    fun encode(
        key: ByteArray,
        deviceId: Int,
        sessionId: Int,
        sequence: Int,
        timestampMs: Long,
        flags: Int,
        heartRate: Int,
        batteryPct: Int,
        restingHr: Int,
    ): ByteArray {
        val header = ByteBuffer.allocate(HEADER_LEN).order(ByteOrder.LITTLE_ENDIAN).apply {
            // Magic is the one big-endian field, so it reads as "PB" in a dump.
            put((MAGIC shr 8).toByte())
            put((MAGIC and 0xFF).toByte())
            put(VERSION)
            put(TYPE_TELEMETRY)
            putInt(deviceId)
            putInt(sessionId)
            putInt(sequence)
            putLong(timestampMs)
        }.array()

        val payload = byteArrayOf(
            flags.toByte(),
            heartRate.coerceIn(0, 255).toByte(),
            batteryPct.coerceIn(0, 255).toByte(),
            restingHr.coerceIn(0, 255).toByte(),
        )

        val nonce = ByteBuffer.allocate(12).order(ByteOrder.LITTLE_ENDIAN).apply {
            putInt(deviceId)
            putInt(sessionId)
            putInt(sequence)
        }.array()

        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(
            Cipher.ENCRYPT_MODE,
            SecretKeySpec(key, "ChaCha20"),
            IvParameterSpec(nonce),
        )
        cipher.updateAAD(header)
        val sealed = cipher.doFinal(payload)

        return header + sealed
    }
}
