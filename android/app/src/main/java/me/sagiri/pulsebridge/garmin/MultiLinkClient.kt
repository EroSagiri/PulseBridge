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
        handler.removeCallbacksAndMessages(null)
        // The close-handle message format is untested, so the client simply
        // detaches. The watch releases the lane when the GATT client goes away.
        gatt?.disconnect()
        gatt?.close()
        gatt = null
        hrHandle = null
    }

    private fun connect() {
        if (stopped) return
        val device = try {
            (context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)
                ?.adapter?.getRemoteDevice(address)
        } catch (e: IllegalArgumentException) {
            Log.w(TAG, "bad address " + address, e)
            null
        } ?: return

        gatt = device.connectGatt(context, true, callback, BluetoothDevice.TRANSPORT_LE)
    }

    private val callback = object : BluetoothGattCallback() {

        override fun onConnectionStateChange(g: BluetoothGatt, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
                    backoffMs = MIN_BACKOFF_MS
                    onConnectionChange(true)
                    onStatus("link up, discovering services")
                    g.discoverServices()
                }

                BluetoothProfile.STATE_DISCONNECTED -> {
                    onConnectionChange(false)
                    hrHandle = null
                    lane = null
                    pending.clear()
                    g.close()
                    gatt = null
                    if (!stopped) {
                        reconnects += 1
                        onStatus("link down, retrying")
                        handler.postDelayed({ connect() }, backoffMs)
                        backoffMs = (backoffMs * 2).coerceAtMost(MAX_BACKOFF_MS)
                    }
                }
            }
        }

        override fun onServicesDiscovered(g: BluetoothGatt, status: Int) {
            val lanes = MultiLink.discoverLanes(g)
            if (lanes.isEmpty()) {
                onStatus("no Multi-Link service on this device")
                Log.w(TAG, "no multi-link lanes found")
                return
            }
            val chosen = lanes.getOrNull(laneIndex)
            if (chosen == null) {
                onStatus("lane $laneIndex not present (${lanes.size} lanes)")
                return
            }
            lane = chosen
            Log.i(TAG, "lanes=" + lanes.size + " using lane " + chosen.index + " " + chosen.notify)

            val notifyChar = g.getService(MultiLink.SERVICE)?.getCharacteristic(chosen.notify)
            if (notifyChar == null) {
                onStatus("lane characteristic missing")
                return
            }
            g.setCharacteristicNotification(notifyChar, true)
            val cccd = notifyChar.getDescriptor(MultiLink.CCCD)
            if (cccd == null) {
                onStatus("lane has no CCCD")
                return
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                g.writeDescriptor(cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
            } else {
                @Suppress("DEPRECATION")
                cccd.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                @Suppress("DEPRECATION")
                g.writeDescriptor(cccd)
            }
        }

        override fun onDescriptorWrite(g: BluetoothGatt, d: BluetoothGattDescriptor, status: Int) {
            if (d.characteristic.uuid != lane?.notify) return
            // Registering REGISTRATION before REAL_TIME_HR mirrors the sequence
            // that was proven to work; GFDI is deliberately not registered,
            // because the watch then repeats device-info frames forever waiting
            // for a handshake this project has no use for.
            pending.clear()
            pending.addLast(MultiLink.SVC_REGISTRATION)
            pending.addLast(MultiLink.SVC_REAL_TIME_HR)
            onStatus("registering services")
            sendNextRegistration(g)
        }

        override fun onCharacteristicChanged(
            g: BluetoothGatt,
            ch: BluetoothGattCharacteristic,
            value: ByteArray,
        ) {
            handleFrame(g, ch, value)
        }

        @Deprecated("Replaced by the ByteArray overload on API 33")
        override fun onCharacteristicChanged(g: BluetoothGatt, ch: BluetoothGattCharacteristic) {
            @Suppress("DEPRECATION")
            val value = ch.value ?: return
            handleFrame(g, ch, value)
        }
    }

    private fun sendNextRegistration(g: BluetoothGatt) {
        val serviceId = pending.firstOrNull() ?: return
        val laneNow = lane ?: return
        val writeChar = g.getService(MultiLink.SERVICE)?.getCharacteristic(laneNow.write) ?: return
        val frame = MultiLink.registrationFrame(serviceId)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
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
            g.writeCharacteristic(writeChar)
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
                onStatus("lane $laneIndex already in use - try another lane")
                pending.clear()
                return
            }

            MultiLink.STATUS_PENDING_AUTH -> {
                onStatus("watch demands authentication on service ${reply.serviceId}")
                pending.clear()
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

    companion object {
        private const val TAG = "MultiLinkClient"
        private const val MIN_BACKOFF_MS = 2_000L
        private const val MAX_BACKOFF_MS = 60_000L
    }
}
