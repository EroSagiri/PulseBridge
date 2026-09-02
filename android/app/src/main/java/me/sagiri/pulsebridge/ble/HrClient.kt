package me.sagiri.pulsebridge.ble

import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothProfile
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import me.sagiri.pulsebridge.HeartRateSource

/**
 * GATT client for the standard Heart Rate Service (0x180D / 0x2A37).
 *
 * Reconnects on its own with a capped backoff, because the link drops every
 * time broadcast mode is toggled on the watch or the wrist goes out of range.
 */
@SuppressLint("MissingPermission")
class HrClient(
    private val context: Context,
    private val address: String,
    private val onSample: (hr: Int, contactOk: Boolean) -> Unit,
    private val onConnectionChange: (connected: Boolean) -> Unit,
) : HeartRateSource {
    private val handler = Handler(Looper.getMainLooper())
    private var gatt: BluetoothGatt? = null
    private var stopped = false
    private var backoffMs = MIN_BACKOFF_MS
    private var reconnectRunnable: Runnable? = null

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
        closeCurrentGatt()
    }

    override fun reconnect(reason: String) {
        handler.post {
            if (stopped) return@post
            Log.w(TAG, "forcing reconnect: $reason")
            onConnectionChange(false)
            closeCurrentGatt()
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

        // autoConnect = true: the stack keeps a background connection attempt
        // alive across the watch going in and out of range, which is cheaper
        // than us re-scanning every time.
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
            handler.post {
                if (g === gatt && status != BluetoothGatt.GATT_SUCCESS) {
                    failCurrentGatt(g, "notification descriptor write failed: $status")
                }
            }
        }

        override fun onCharacteristicChanged(
            g: BluetoothGatt,
            ch: BluetoothGattCharacteristic,
            value: ByteArray,
        ) {
            val copy = value.copyOf()
            handler.post { if (g === gatt) handle(ch, copy) }
        }

        @Deprecated("Replaced by the ByteArray overload on API 33")
        override fun onCharacteristicChanged(g: BluetoothGatt, ch: BluetoothGattCharacteristic) {
            @Suppress("DEPRECATION")
            val value = ch.value?.copyOf() ?: return
            handler.post { if (g === gatt) handle(ch, value) }
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
                g.requestConnectionPriority(BluetoothGatt.CONNECTION_PRIORITY_LOW_POWER)
                if (!g.discoverServices()) {
                    failCurrentGatt(g, "discoverServices returned false")
                }
            }

            BluetoothProfile.STATE_DISCONNECTED -> {
                gatt = null
                g.close()
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
        val ch = g.getService(GattUuids.HEART_RATE_SERVICE)
            ?.getCharacteristic(GattUuids.HEART_RATE_MEASUREMENT)
        if (ch == null) {
            failCurrentGatt(g, "heart-rate characteristic missing")
            return
        }
        if (!g.setCharacteristicNotification(ch, true)) {
            failCurrentGatt(g, "setCharacteristicNotification returned false")
            return
        }
        val cccd = ch.getDescriptor(GattUuids.CLIENT_CHARACTERISTIC_CONFIG)
        if (cccd == null) {
            failCurrentGatt(g, "heart-rate CCCD missing")
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

    private fun failCurrentGatt(g: BluetoothGatt, reason: String) {
        if (g !== gatt) return
        Log.w(TAG, reason)
        onConnectionChange(false)
        closeCurrentGatt()
        scheduleReconnect(reason)
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
        Log.i(TAG, "retrying in ${delayMs}ms: $reason")
        val task = Runnable {
            reconnectRunnable = null
            connect()
        }
        reconnectRunnable = task
        handler.postDelayed(task, delayMs)
        if (delayMs > 0L) backoffMs = (backoffMs * 2).coerceAtMost(MAX_BACKOFF_MS)
    }

    /*
     * BLE callbacks are marshalled onto [handler] above. This keeps teardown,
     * reconnect scheduling and notification setup ordered on one thread; a
     * callback from a closed GATT instance cannot revive stale state.
     */

    private fun handle(ch: BluetoothGattCharacteristic, value: ByteArray) {
        if (ch.uuid != GattUuids.HEART_RATE_MEASUREMENT) return
        val parsed = parseMeasurement(value) ?: return
        samples += 1
        onSample(parsed.first, parsed.second)
    }

    companion object {
        private const val TAG = "HrClient"
        private const val MIN_BACKOFF_MS = 1_000L
        private const val MAX_BACKOFF_MS = 30_000L

        /**
         * Heart Rate Measurement, Bluetooth SIG spec:
         *   flags bit0  value format, 0 = uint8 and 1 = uint16
         *   flags bit1  sensor contact detected
         *   flags bit2  sensor contact supported
         *
         * Returns null for a truncated packet. Contact counts as ok when the
         * sensor does not support contact detection at all, otherwise a strap
         * that never sets the bit would look permanently faulty.
         */
        fun parseMeasurement(value: ByteArray): Pair<Int, Boolean>? {
            if (value.isEmpty()) return null
            val flags = value[0].toInt() and 0xFF
            val wide = flags and 0x01 != 0
            val hr = if (wide) {
                if (value.size < 3) return null
                (value[1].toInt() and 0xFF) or ((value[2].toInt() and 0xFF) shl 8)
            } else {
                if (value.size < 2) return null
                value[1].toInt() and 0xFF
            }
            if (hr < 1 || hr > 255) return null
            val contactSupported = flags and 0x04 != 0
            val contactDetected = flags and 0x02 != 0
            return Pair(hr, !contactSupported || contactDetected)
        }
    }
}
