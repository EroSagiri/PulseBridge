package me.sagiri.pulsebridge.garmin

import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import me.sagiri.pulsebridge.HeartRateSource

/**
 * Streams 1 Hz heart rate off a Garmin watch over its private Multi-Link
 * channel, alongside a running Garmin Connect rather than instead of it.
 *
 * Android multiplexes GATT clients from different apps over one ACL link, so
 * this attaches to the link Connect already holds. Measured on a Forerunner
 * 255: Connect kept its `gatt_if` throughout and never reconnected. That only
 * covers two apps on the same phone -- a second phone is a different problem.
 *
 * Registration needs no pairing and no authentication as long as the lane is
 * unclaimed and the client uuid does not collide with Connect's.
 */
@SuppressLint("MissingPermission")
class MultiLinkClient(
    private val context: Context,
    private val address: String,
    /** Which discovered lane to claim. Connect was observed on lane 1. */
    private val laneIndex: Int,
    private val onSample: (hr: Int, restingHr: Int?) -> Unit,
    private val onConnectionChange: (connected: Boolean) -> Unit,
    private val onStatus: (String) -> Unit,
) : HeartRateSource {

    private val handler = Handler(Looper.getMainLooper())
    private var gatt: BluetoothGatt? = null
    private var stopped = false
    private var backoffMs = MIN_BACKOFF_MS
    private var reconnectRunnable: Runnable? = null

    private var lane: MultiLink.Lane? = null
    private var hrHandle: Int? = null

    /** Registration is strictly one write at a time; the reply drives the next. */
    private val pending = ArrayDeque<Int>()

    @Volatile
    override var reconnects: Int = 0
        private set

    @Volatile
    override var samples: Long = 0
        private set

    override fun start() {
        stopped = false
        connect()
    }

    override fun stop() {
        stopped = true
        reconnectRunnable?.let(handler::removeCallbacks)
        reconnectRunnable = null
        // The close-handle message format is untested, so the client simply
        // detaches. The watch releases the lane when the GATT client goes away.
        closeCurrentGatt()
        clearRegistrationState()
    }

    override fun reconnect(reason: String) {
        handler.post {
            if (stopped) return@post
            Log.w(TAG, "forcing reconnect: $reason")
            onStatus("stream stalled, reconnecting")
            onConnectionChange(false)
            closeCurrentGatt()
            clearRegistrationState()
            scheduleReconnect(reason, delayMs = 0L)
        }
    }

    private fun connect() {
        if (stopped || gatt != null) return
        val device = try {
            (context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)
                ?.adapter?.getRemoteDevice(address)
        } catch (e: Exception) {
            Log.w(TAG, "cannot resolve device $address", e)
            null
        }
        if (device == null) {
            scheduleReconnect("device unavailable")
            return
        }

        onStatus("connecting")
        try {
            gatt = device.connectGatt(context, true, callback, BluetoothDevice.TRANSPORT_LE)
        } catch (e: Exception) {
            Log.w(TAG, "connectGatt failed", e)
            scheduleReconnect("connectGatt failed")
        }
    }

    private val callback = object : BluetoothGattCallback() {

        override fun onConnectionStateChange(g: BluetoothGatt, status: Int, newState: Int) {
            handler.post { handleConnectionStateChange(g, status, newState) }
        }

        override fun onServicesDiscovered(g: BluetoothGatt, status: Int) {
            handler.post { handleServicesDiscovered(g, status) }
        }

        override fun onDescriptorWrite(g: BluetoothGatt, d: BluetoothGattDescriptor, status: Int) {
            handler.post { handleDescriptorWrite(g, d, status) }
        }

        override fun onCharacteristicChanged(
            g: BluetoothGatt,
            ch: BluetoothGattCharacteristic,
            value: ByteArray,
        ) {
            val copy = value.copyOf()
            handler.post { if (g === gatt) handleFrame(g, ch, copy) }
        }

        @Deprecated("Replaced by the ByteArray overload on API 33")
        override fun onCharacteristicChanged(g: BluetoothGatt, ch: BluetoothGattCharacteristic) {
            @Suppress("DEPRECATION")
            val value = ch.value?.copyOf() ?: return
            handler.post { if (g === gatt) handleFrame(g, ch, value) }
        }
    }

    private fun handleConnectionStateChange(g: BluetoothGatt, status: Int, newState: Int) {
        if (g !== gatt) {
            g.close()
            return
        }
        if (status != BluetoothGatt.GATT_SUCCESS && newState != BluetoothProfile.STATE_DISCONNECTED) {
            failCurrentGatt(g, "connection state failed: $status")
            return
        }
        when (newState) {
            BluetoothProfile.STATE_CONNECTED -> {
                backoffMs = MIN_BACKOFF_MS
                onConnectionChange(true)
                onStatus("link up, discovering services")
                if (!g.discoverServices()) {
                    failCurrentGatt(g, "discoverServices returned false")
                }
            }

            BluetoothProfile.STATE_DISCONNECTED -> {
                gatt = null
                g.close()
                clearRegistrationState()
                onConnectionChange(false)
                scheduleReconnect("link down")
            }
        }
    }

    private fun handleServicesDiscovered(g: BluetoothGatt, status: Int) {
        if (g !== gatt) return
        if (status != BluetoothGatt.GATT_SUCCESS) {
            failCurrentGatt(g, "service discovery failed: $status")
            return
        }
        val lanes = MultiLink.discoverLanes(g)
        if (lanes.isEmpty()) {
            failCurrentGatt(g, "no Multi-Link service on this device")
            return
        }
        val chosen = lanes.getOrNull(laneIndex)
        if (chosen == null) {
            failCurrentGatt(g, "lane $laneIndex not present (${lanes.size} lanes)")
            return
        }
        lane = chosen
        Log.i(TAG, "lanes=${lanes.size} using lane ${chosen.index} ${chosen.notify}")

        val notifyChar = g.getService(MultiLink.SERVICE)?.getCharacteristic(chosen.notify)
        if (notifyChar == null) {
            failCurrentGatt(g, "lane characteristic missing")
            return
        }
        if (!g.setCharacteristicNotification(notifyChar, true)) {
            failCurrentGatt(g, "setCharacteristicNotification returned false")
            return
        }
        val cccd = notifyChar.getDescriptor(MultiLink.CCCD)
        if (cccd == null) {
            failCurrentGatt(g, "lane has no CCCD")
            return
        }
        val rc = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            g.writeDescriptor(cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
        } else {
            @Suppress("DEPRECATION")
            cccd.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
            @Suppress("DEPRECATION")
            if (g.writeDescriptor(cccd)) BluetoothGatt.GATT_SUCCESS else -1
        }
        if (rc != BluetoothGatt.GATT_SUCCESS) {
            failCurrentGatt(g, "writeDescriptor returned $rc")
        }
    }

    private fun handleDescriptorWrite(
        g: BluetoothGatt,
        d: BluetoothGattDescriptor,
        status: Int,
    ) {
        if (g !== gatt || d.characteristic.uuid != lane?.notify) return
        if (status != BluetoothGatt.GATT_SUCCESS) {
            failCurrentGatt(g, "notification descriptor write failed: $status")
            return
        }
        // Registering REGISTRATION before REAL_TIME_HR mirrors the sequence
        // that was proven to work; GFDI is deliberately not registered.
        pending.clear()
        pending.addLast(MultiLink.SVC_REGISTRATION)
        pending.addLast(MultiLink.SVC_REAL_TIME_HR)
        onStatus("registering services")
        sendNextRegistration(g)
    }

    private fun sendNextRegistration(g: BluetoothGatt) {
        if (g !== gatt) return
        val serviceId = pending.firstOrNull() ?: return
        val laneNow = lane
        if (laneNow == null) {
            failCurrentGatt(g, "registration lane disappeared")
            return
        }
        val writeChar = g.getService(MultiLink.SERVICE)?.getCharacteristic(laneNow.write)
        if (writeChar == null) {
            failCurrentGatt(g, "registration write characteristic missing")
            return
        }
        val frame = MultiLink.registrationFrame(serviceId)

        val rc = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            g.writeCharacteristic(
                writeChar,
                frame,
                BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT,
            )
        } else {
            @Suppress("DEPRECATION")
            writeChar.value = frame
            @Suppress("DEPRECATION")
            writeChar.writeType = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
            @Suppress("DEPRECATION")
            if (g.writeCharacteristic(writeChar)) BluetoothGatt.GATT_SUCCESS else -1
        }
        if (rc != BluetoothGatt.GATT_SUCCESS) {
            failCurrentGatt(g, "registration write returned $rc")
        }
    }

    private fun handleFrame(g: BluetoothGatt, ch: BluetoothGattCharacteristic, value: ByteArray) {
        if (ch.uuid != lane?.notify) return

        MultiLink.parseRegistrationReply(value)?.let { reply ->
            onRegistrationReply(g, reply)
            return
        }

        val handle = hrHandle ?: return
        val frame = MultiLink.parseHrFrame(handle, value) ?: return
        samples += 1
        onSample(frame.heartRate, frame.restingHr)
    }

    private fun onRegistrationReply(g: BluetoothGatt, reply: MultiLink.RegistrationReply) {
        Log.i(
            TAG,
            "register svc=" + reply.serviceId +
                " status=" + MultiLink.statusName(reply.status) +
                " handle=" + reply.handle
        )
        if (pending.firstOrNull() == reply.serviceId) pending.removeFirst()

        when (reply.status) {
            MultiLink.STATUS_SUCCESS -> {
                if (reply.serviceId == MultiLink.SVC_REAL_TIME_HR) {
                    // Handles are assigned at registration time in registration
                    // order, so this must be taken from the reply and never
                    // assumed to be a constant.
                    hrHandle = reply.handle
                    onStatus("streaming, hr handle 0x%02x".format(reply.handle ?: 0))
                }
            }

            MultiLink.STATUS_ALREADY_IN_USE -> {
                // Another client owns this lane. Moving to a different lane is
                // left to the user rather than done automatically: writing into
                // the lane Garmin Connect holds is the one thing that could
                // disturb it.
                stopForConfigurationError(g, "lane $laneIndex already in use - try another lane")
                return
            }

            MultiLink.STATUS_PENDING_AUTH -> {
                stopForConfigurationError(
                    g,
                    "watch demands authentication on service ${reply.serviceId}",
                )
                return
            }

            else -> {
                onStatus(
                    "service ${reply.serviceId} rejected: ${MultiLink.statusName(reply.status)}"
                )
            }
        }
        if (pending.isNotEmpty()) sendNextRegistration(g)
    }

    private fun stopForConfigurationError(g: BluetoothGatt, reason: String) {
        if (g !== gatt) return
        onStatus(reason)
        onConnectionChange(false)
        closeCurrentGatt()
        clearRegistrationState()
    }

    private fun failCurrentGatt(g: BluetoothGatt, reason: String) {
        if (g !== gatt) return
        Log.w(TAG, reason)
        onStatus(reason)
        onConnectionChange(false)
        closeCurrentGatt()
        clearRegistrationState()
        scheduleReconnect(reason)
    }

    private fun clearRegistrationState() {
        hrHandle = null
        lane = null
        pending.clear()
    }

    private fun closeCurrentGatt() {
        val current = gatt ?: return
        gatt = null
        runCatching { current.disconnect() }
        current.close()
    }

    private fun scheduleReconnect(reason: String, delayMs: Long = backoffMs) {
        if (stopped || reconnectRunnable != null) return
        reconnects += 1
        onStatus("retrying in ${delayMs}ms: $reason")
        val task = Runnable {
            reconnectRunnable = null
            connect()
        }
        reconnectRunnable = task
        handler.postDelayed(task, delayMs)
        if (delayMs > 0L) backoffMs = (backoffMs * 2).coerceAtMost(MAX_BACKOFF_MS)
    }

    companion object {
        private const val TAG = "MultiLinkClient"
        private const val MIN_BACKOFF_MS = 2_000L
        private const val MAX_BACKOFF_MS = 60_000L
    }
}
