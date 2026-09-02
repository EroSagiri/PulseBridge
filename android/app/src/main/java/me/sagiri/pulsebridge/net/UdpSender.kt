package me.sagiri.pulsebridge.net

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
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

    @Volatile private var pending: Sample? = null
    @Volatile private var lastSentAtMs: Long = 0
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
            try {
                socket = DatagramSocket()
                // Resolution happens once here rather than per packet; a server
                // IP change is handled by restarting the service.
                address = InetAddress.getByName(host)
            } catch (e: Exception) {
                lastError = "resolve/bind failed: ${e.message}"
                Log.w(TAG, "cannot open socket", e)
                return@launch
            }
            while (true) {
                val sample = pending
                if (sample != null) {
                    val flags = flagsOf(sample, heartbeat = false)
                    val changed = sample.heartRate != lastSentHr || flags != lastSentFlags
                    val due = System.currentTimeMillis() - lastSentAtMs >= HEARTBEAT_MS
                    if (changed || due) {
                        send(sample, heartbeat = !changed)
                    }
                }
                delay(TICK_MS)
            }
        }
    }

    /** Called from the BLE callback thread; the send loop picks it up. */
    fun offer(sample: Sample) {
        pending = sample
    }

    private suspend fun send(sample: Sample, heartbeat: Boolean) = withContext(Dispatchers.IO) {
        val sock = socket ?: return@withContext
        val addr = address ?: return@withContext
        sequence += 1
        val flags = flagsOf(sample, heartbeat)
        try {
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
            lastSentAtMs = System.currentTimeMillis()
            lastSentHr = sample.heartRate ?: -1
            lastSentFlags = flags
            lastError = null
        } catch (e: Exception) {
            // A send failure on a mobile network is routine (handover, doze
            // wakeup race). Record it and let the next tick try again.
            lastError = e.message
            Log.d(TAG, "send failed", e)
        }
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
        socket?.close()
        socket = null
    }

    companion object {
        private const val TAG = "UdpSender"
        /** Keeps carrier NAT bindings alive; they can expire at 15-30 s. */
        const val HEARTBEAT_MS = 10_000L
        private const val TICK_MS = 250L
    }
}
