package me.sagiri.pulsebridge

import android.content.Context
import java.security.SecureRandom

/**
 * Plain SharedPreferences on purpose: the pre-shared key is the only secret and
 * it is useless without also owning the device it authenticates. Swapping in
 * EncryptedSharedPreferences later touches only this file.
 */
class Prefs(context: Context) {

    private val sp = context.getSharedPreferences("pulsebridge", Context.MODE_PRIVATE)

    var serverHost: String
        get() = sp.getString(KEY_HOST, "") ?: ""
        set(v) = sp.edit().putString(KEY_HOST, v.trim()).apply()

    var serverPort: Int
        get() = sp.getInt(KEY_PORT, 9999)
        set(v) = sp.edit().putInt(KEY_PORT, v).apply()

    var deviceId: Int
        get() {
            val existing = sp.getInt(KEY_DEVICE_ID, 0)
            if (existing != 0) return existing
            val generated = SecureRandom().nextInt(Int.MAX_VALUE) + 1
            sp.edit().putInt(KEY_DEVICE_ID, generated).apply()
            return generated
        }
        set(v) = sp.edit().putInt(KEY_DEVICE_ID, v).apply()

    var keyHex: String
        get() = sp.getString(KEY_KEY, "") ?: ""
        set(v) = sp.edit().putString(KEY_KEY, v.trim().lowercase()).apply()

    var watchAddress: String?
        get() = sp.getString(KEY_WATCH_ADDR, null)
        set(v) = sp.edit().putString(KEY_WATCH_ADDR, v).apply()

    var watchName: String?
        get() = sp.getString(KEY_WATCH_NAME, null)
        set(v) = sp.edit().putString(KEY_WATCH_NAME, v).apply()

    /** Which feed to use. Multi-Link is the default now that it is proven. */
    var sourceMode: SourceMode
        get() = SourceMode.from(sp.getString(KEY_SOURCE, null))
        set(v) = sp.edit().putString(KEY_SOURCE, v.name).apply()

    /**
     * Multi-Link lane to claim. Garmin Connect was observed holding lane 1 on a
     * Forerunner 255, so lane 0 is the default and moving off it is manual --
     * writing into Connect's lane is the one action that could disturb it.
     */
    var laneIndex: Int
        get() = sp.getInt(KEY_LANE, 0)
        set(v) = sp.edit().putInt(KEY_LANE, v).apply()

    /** Whether the service should come back up after a reboot. */
    var autoStart: Boolean
        get() = sp.getBoolean(KEY_AUTOSTART, false)
        set(v) = sp.edit().putBoolean(KEY_AUTOSTART, v).apply()

    fun isConfigured(): Boolean =
        serverHost.isNotEmpty() && keyHex.length == 64 && watchAddress != null

    companion object {
        private const val KEY_HOST = "server_host"
        private const val KEY_PORT = "server_port"
        private const val KEY_DEVICE_ID = "device_id"
        private const val KEY_KEY = "key_hex"
        private const val KEY_WATCH_ADDR = "watch_addr"
        private const val KEY_WATCH_NAME = "watch_name"
        private const val KEY_AUTOSTART = "auto_start"
        private const val KEY_SOURCE = "source_mode"
        private const val KEY_LANE = "lane_index"
    }
}
