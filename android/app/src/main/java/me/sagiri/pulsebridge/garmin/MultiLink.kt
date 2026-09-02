package me.sagiri.pulsebridge.garmin

import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import java.util.Locale
import java.util.UUID

/**
 * Garmin Multi-Link framing, as measured on a Forerunner 255 rather than taken
 * from documentation. See docs/phase0-multilink.md for the raw captures.
 *
 * Everything here is pure so the frame formats can be reasoned about and tested
 * without a watch attached.
 */
object MultiLink {

    /** Garmin 128-bit UUID base: 6a4eXXXX-667b-11e3-949a-0800200c9a66 */
    fun garmin(shortId: Int): UUID = UUID.fromString(
        String.format(Locale.US, "6a4e%04x-667b-11e3-949a-0800200c9a66", shortId)
    )

    val SERVICE: UUID = garmin(0x2800)

    /** Capability queries; claims nothing and is safe to read at any time. */
    val REGISTRATION_CHAR: UUID = garmin(0x2803)

    val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")

    // Service ids from the FR255 capability bitmap. Only the ones we actually
    // use are named; the rest stay numbers until something confirms them.
    const val SVC_GFDI = 1
    const val SVC_REGISTRATION = 4
    const val SVC_REAL_TIME_HR = 6

    const val STATUS_SUCCESS = 0x00
    const val STATUS_INVALID_SERVICE_ID = 0x01
    const val STATUS_PENDING_AUTH = 0x02
    const val STATUS_ALREADY_IN_USE = 0x03
    const val STATUS_REJECTED = 0x04

    fun statusName(status: Int): String = when (status) {
        STATUS_SUCCESS -> "SUCCESS"
        STATUS_INVALID_SERVICE_ID -> "INVALID_SERVICE_ID"
        STATUS_PENDING_AUTH -> "PENDING_AUTH"
        STATUS_ALREADY_IN_USE -> "ALREADY_IN_USE"
        STATUS_REJECTED -> "REJECTED"
        else -> "UNKNOWN(0x%02x)".format(status)
    }

    /**
     * Identifies this client to the watch. Anything but 0x01, which is what
     * Garmin Connect uses -- colliding with it is what would disturb Connect.
     */
    val CLIENT_UUID = byteArrayOf(0x50, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00)

    /**
     * A Multi-Link lane: one notify characteristic to read from and one write
     * characteristic to send on. On the FR255 these are 2810/2820, 2811/2821
     * and 2830/2840 -- note the third pair is not the 2812/2822 the public
     * notes suggest, which is why lanes are discovered rather than hardcoded.
     */
    data class Lane(val index: Int, val notify: UUID, val write: UUID)

    /**
     * Finds the lanes a device actually exposes. The write characteristic of a
     * lane sits 0x10 above its notify characteristic, which holds for all three
     * observed pairs; anything not matching that is skipped rather than guessed.
     */
    fun discoverLanes(gatt: BluetoothGatt): List<Lane> {
        val service = gatt.getService(SERVICE) ?: return emptyList()
        val present = service.characteristics.mapNotNull { ch ->
            shortId(ch.uuid)?.let { it to ch }
        }.toMap()

        // A lane is a notifying characteristic that has a matching write
        // characteristic 0x10 above it. Write characteristics do not notify,
        // so this picks out exactly the readable side of each pair no matter
        // how the device numbers them.
        return present.keys
            .filter { it in 0x2810..0x28FF }
            .filter { id ->
                val ch = present[id] ?: return@filter false
                ch.properties and BluetoothGattCharacteristic.PROPERTY_NOTIFY != 0 &&
                    present.containsKey(id + 0x10)
            }
            .sorted()
            .mapIndexed { index, id -> Lane(index, garmin(id), garmin(id + 0x10)) }
    }

    private fun shortId(uuid: UUID): Int? {
        val s = uuid.toString()
        if (!s.startsWith("6a4e") || !s.endsWith("-667b-11e3-949a-0800200c9a66")) return null
        return s.substring(4, 8).toIntOrNull(16)
    }

    /**
     * Handle registration request, written to a lane's write characteristic:
     *
     *     00 00 | client_uuid[8] | service_id[2, LE] | 00
     */
    fun registrationFrame(serviceId: Int): ByteArray {
        val frame = ByteArray(13)
        frame[0] = 0x00
        frame[1] = 0x00
        CLIENT_UUID.copyInto(frame, 2)
        frame[10] = (serviceId and 0xFF).toByte()
        frame[11] = ((serviceId shr 8) and 0xFF).toByte()
        frame[12] = 0x00
        return frame
    }

    data class RegistrationReply(val serviceId: Int, val status: Int, val handle: Int?)

    /**
     * Reply, arriving as a notification on the same lane:
     *
     *     00 01 | client_uuid[8] | service_id[2, LE] | status | handle | ...
     *
     * Returns null for anything that is not a registration reply, which is how
     * data frames are told apart from control frames on the same lane.
     */
    fun parseRegistrationReply(v: ByteArray): RegistrationReply? {
        if (v.size < 13) return null
        if (v[0].toInt() != 0x00 || v[1].toInt() != 0x01) return null
        val serviceId = (v[10].toInt() and 0xFF) or ((v[11].toInt() and 0xFF) shl 8)
        val status = v[12].toInt() and 0xFF
        val handle = if (v.size > 13) v[13].toInt() and 0xFF else null
        return RegistrationReply(serviceId, status, handle)
    }

    data class HrFrame(val heartRate: Int, val restingHr: Int?)

    /**
     * Real-time heart rate frame, e.g. `24 03 42 33 ff ff`:
     *
     *     [0] handle assigned at registration
     *     [1] 0x03 -- constant across every sample captured so far
     *     [2] current heart rate, bpm
     *     [3] resting heart rate, bpm
     *     [4..5] 16-bit field, 0xffff seen throughout, meaning unknown
     *
     * Offsets 1 and 4-5 come from a single resting session and are not
     * confirmed, so this only reads the two bytes that were cross-checked
     * against Garmin Connect and refuses anything shorter.
     */
    fun parseHrFrame(handle: Int, v: ByteArray): HrFrame? {
        if (v.size < 4) return null
        if ((v[0].toInt() and 0xFF) != handle) return null
        val hr = v[2].toInt() and 0xFF
        if (hr == 0 || hr == 0xFF) return null
        val resting = v[3].toInt() and 0xFF
        return HrFrame(hr, if (resting == 0 || resting == 0xFF) null else resting)
    }

    /** True when the frame is addressed to a handle we registered. */
    fun handleOf(v: ByteArray): Int? =
        if (v.isEmpty()) null else v[0].toInt() and 0xFF
}
