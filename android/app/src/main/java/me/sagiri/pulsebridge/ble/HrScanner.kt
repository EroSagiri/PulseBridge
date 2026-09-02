package me.sagiri.pulsebridge.ble

import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.content.Context
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.os.ParcelUuid

/**
 * Scans for anything advertising the standard Heart Rate Service. A Garmin in
 * broadcast mode, a chest strap and an armband all look identical here, which
 * is the whole point of using the standard profile.
 */
@SuppressLint("MissingPermission")
class HrScanner(
    private val context: Context,
    private val onFound: (address: String, name: String) -> Unit,
) {

    private fun adapter() =
        (context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter

    private var callback: ScanCallback? = null

    fun start(): Boolean {
        val scanner = adapter()?.bluetoothLeScanner ?: return false
        if (callback != null) return true

        val cb = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) {
                val name = result.device.name ?: result.scanRecord?.deviceName ?: "(unnamed)"
                onFound(result.device.address, name)
            }
        }
        val filter = ScanFilter.Builder()
            .setServiceUuid(ParcelUuid(GattUuids.HEART_RATE_SERVICE))
            .build()
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .build()

        scanner.startScan(listOf(filter), settings, cb)
        callback = cb
        return true
    }

    fun stop() {
        val scanner = adapter()?.bluetoothLeScanner
        callback?.let { scanner?.stopScan(it) }
        callback = null
    }
}
