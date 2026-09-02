package me.sagiri.pulsebridge.net

import android.os.SystemClock
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.security.SecureRandom

/**
 * Owns the send policy from protocol.md section 6: fire on change, otherwise a
 * heartbeat every [HEARTBEAT_MS]. Nothing is queued or retransmitted -- for
 * live telemetry the newest value always supersedes the one that was lost.
 */
class UdpSender(
    private val host: String,
    private val port: Int,
    private val deviceId: Int,
    key: ByteArray,
    private val scope: CoroutineScope,
) {
    private val key = key.copyOf()
    private val sessionId: Int = SecureRandom().nextInt()
    private var sequence: Int = 0

    private var socket: DatagramSocket? = null
    private var address: InetAddress? = null
    private var job: Job? = null
    private val updates = Channel<Sample>(Channel.CONFLATED)
    private var openBackoffMs = MIN_OPEN_BACKOFF_MS

    @Volatile private var lastSentAtElapsedMs: Long = 0
    @Volatile private var lastSentHr: Int = -1
    @Volatile private var lastSentFlags: Int = -1

    @Volatile var packetsSent: Long = 0; private set
    @Volatile var lastError: String? = null; private set

    data class Sample(
        val heartRate: Int?,
        val contactOk: Boolean,
        val watchConnected: Boolean,
        val batteryPct: Int,
        /** Only Multi-Link supplies this; 0 means unknown. */
        val restingHr: Int,
    )

    fun start() {
        if (job != null) return
        job = scope.launch(Dispatchers.IO) {
            var latest: Sample? = null
            while (currentCoroutineContext().isActive) {
                val waitMs = latest?.let {
                    (HEARTBEAT_MS - (SystemClock.elapsedRealtime() - lastSentAtElapsedMs))
                        .coerceAtLeast(0L)
                }
                val incoming = when {
                    latest == null -> updates.receive()
                    waitMs != null && waitMs > 0L -> withTimeoutOrNull(waitMs) { updates.receive() }
                    else -> null
                }
                if (incoming != null) latest = incoming

                val sample = latest ?: continue
                val flags = flagsOf(sample, heartbeat = false)
                val changed = sample.heartRate != lastSentHr || flags != lastSentFlags
                val due = lastSentAtElapsedMs == 0L ||
                    SystemClock.elapsedRealtime() - lastSentAtElapsedMs >= HEARTBEAT_MS
                if ((changed || due) && !send(sample, heartbeat = !changed)) {
                    delay(openBackoffMs)
                }
            }
        }
    }

    /** Called from the BLE callback thread; the conflated channel keeps only the newest value. */
    fun offer(sample: Sample) {
        updates.trySend(sample)
    }

    private fun send(sample: Sample, heartbeat: Boolean): Boolean {
        if (!ensureSocket()) return false
        val sock = socket ?: return false
        val addr = address ?: return false
        sequence += 1
        val flags = flagsOf(sample, heartbeat)
        return try {
            val bytes = PacketCodec.encode(
                key = key,
                deviceId = deviceId,
                sessionId = sessionId,
                sequence = sequence,
                timestampMs = System.currentTimeMillis(),
                flags = flags,
                heartRate = sample.heartRate ?: 0,
                batteryPct = sample.batteryPct,
                restingHr = sample.restingHr,
            )
            sock.send(DatagramPacket(bytes, bytes.size, addr, port))
            packetsSent += 1
            lastSentAtElapsedMs = SystemClock.elapsedRealtime()
            lastSentHr = sample.heartRate ?: -1
            // HEARTBEAT describes this packet, not the underlying sample. Do
            // not let it make the same sample look changed on the next loop.
            lastSentFlags = flagsOf(sample, heartbeat = false)
            lastError = null
            openBackoffMs = MIN_OPEN_BACKOFF_MS
            true
        } catch (e: Exception) {
            lastError = "send failed: ${e.message}"
            Log.d(TAG, "send failed", e)
            closeSocket()
            openBackoffMs = (openBackoffMs * 2).coerceAtMost(MAX_OPEN_BACKOFF_MS)
            false
        }
    }

    private fun ensureSocket(): Boolean {
        if (socket != null && address != null) return true
        return try {
            val resolved = InetAddress.getByName(host)
            val opened = DatagramSocket()
            address = resolved
            socket = opened
            true
        } catch (e: Exception) {
            lastError = "resolve/bind failed: ${e.message}"
            Log.w(TAG, "cannot open socket", e)
            closeSocket()
            openBackoffMs = (openBackoffMs * 2).coerceAtMost(MAX_OPEN_BACKOFF_MS)
            false
        }
    }

    private fun closeSocket() {
        socket?.close()
        socket = null
        address = null
    }

    private fun flagsOf(s: Sample, heartbeat: Boolean): Int {
        var f = 0
        if (s.heartRate != null) f = f or PacketCodec.FLAG_HR_VALID
        if (s.contactOk) f = f or PacketCodec.FLAG_CONTACT_OK
        if (s.watchConnected) f = f or PacketCodec.FLAG_WATCH_CONNECTED
        if (heartbeat) f = f or PacketCodec.FLAG_HEARTBEAT
        return f
    }

    fun stop() {
        job?.cancel()
        job = null
        closeSocket()
    }

    companion object {
        private const val TAG = "UdpSender"
        /** Keeps carrier NAT bindings alive; they can expire at 15-30 s. */
        const val HEARTBEAT_MS = 10_000L
        private const val MIN_OPEN_BACKOFF_MS = 1_000L
        private const val MAX_OPEN_BACKOFF_MS = 60_000L
    }
}
