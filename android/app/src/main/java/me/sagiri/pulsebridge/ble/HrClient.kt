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
) {
    private val handler = Handler(Looper.getMainLooper())
    private var gatt: BluetoothGatt? = null
    private var stopped = false
    private var backoffMs = MIN_BACKOFF_MS

    @Volatile
    var reconnects: Int = 0
        private set

    @Volatile
    var samples: Long = 0
        private set

    fun start() {
        stopped = false
        connect()
    }

    fun stop() {
        stopped = true
        handler.removeCallbacksAndMessages(null)
        gatt?.disconnect()
        gatt?.close()
        gatt = null
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

        // autoConnect = true: the stack keeps a background connection attempt
        // alive across the watch going in and out of range, which is cheaper
        // than us re-scanning every time.
        gatt = device.connectGatt(context, true, callback, BluetoothDevice.TRANSPORT_LE)
    }

    private val callback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(g: BluetoothGatt, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
                    backoffMs = MIN_BACKOFF_MS
                    onConnectionChange(true)
                    // A longer connection interval is the biggest phone-side
                    // power saving available. The peripheral still pushes
                    // notifications at its own 1 Hz.
                    g.requestConnectionPriority(BluetoothGatt.CONNECTION_PRIORITY_LOW_POWER)
                    g.discoverServices()
                }

                BluetoothProfile.STATE_DISCONNECTED -> {
                    onConnectionChange(false)
                    g.close()
                    gatt = null
                    if (!stopped) {
                        reconnects += 1
                        handler.postDelayed({ connect() }, backoffMs)
                        backoffMs = (backoffMs * 2).coerceAtMost(MAX_BACKOFF_MS)
                    }
                }
            }
        }

        override fun onServicesDiscovered(g: BluetoothGatt, status: Int) {
            val ch = g.getService(GattUuids.HEART_RATE_SERVICE)
                ?.getCharacteristic(GattUuids.HEART_RATE_MEASUREMENT)
            if (ch == null) {
                Log.w(TAG, "no heart rate characteristic - is broadcast mode still on?")
                return
            }
            g.setCharacteristicNotification(ch, true)
            val cccd = ch.getDescriptor(GattUuids.CLIENT_CHARACTERISTIC_CONFIG) ?: return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                g.writeDescriptor(cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
            } else {
                @Suppress("DEPRECATION")
                cccd.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                @Suppress("DEPRECATION")
                g.writeDescriptor(cccd)
            }
        }

        override fun onCharacteristicChanged(
            g: BluetoothGatt,
            ch: BluetoothGattCharacteristic,
            value: ByteArray,
        ) {
            handle(ch, value)
        }

        @Deprecated("Replaced by the ByteArray overload on API 33")
        override fun onCharacteristicChanged(g: BluetoothGatt, ch: BluetoothGattCharacteristic) {
            @Suppress("DEPRECATION")
            val value = ch.value ?: return
            handle(ch, value)
        }
    }

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
