package me.sagiri.pulsebridge.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.bluetooth.BluetoothManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import me.sagiri.pulsebridge.BridgeState
import me.sagiri.pulsebridge.HeartRateSource
import me.sagiri.pulsebridge.MainActivity
import me.sagiri.pulsebridge.Prefs
import me.sagiri.pulsebridge.PbLog
import me.sagiri.pulsebridge.SourceMode
import me.sagiri.pulsebridge.ble.HrClient
import me.sagiri.pulsebridge.garmin.MultiLinkClient
import me.sagiri.pulsebridge.net.PacketCodec
import me.sagiri.pulsebridge.net.UdpSender

/**
 * Foreground service that owns the whole pipeline: BLE notifications in, UDP
 * telemetry out. A foreground service of type `connectedDevice` is what keeps
 * both the BLE link and network access alive once the phone enters Doze.
 */
class BridgeService : LifecycleService() {

    private lateinit var prefs: Prefs
    private var source: HeartRateSource? = null
    private var sender: UdpSender? = null
    private var startupJob: Job? = null
    private var refreshJob: Job? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var screenReceiverRegistered = false
    private val streamWatchdog = StreamWatchdog(
        stallAfterMs = STREAM_STALL_AFTER_MS,
        recoveryCooldownMs = STREAM_RECOVERY_COOLDOWN_MS,
    )

    @Volatile
    private var lastHr: Int? = null

    @Volatile
    private var restingHr: Int? = null

    @Volatile
    private var contactOk = false

    @Volatile
    private var watchConnected = false

    /** Set on the BLE callback thread, so freshness does not depend on the UI state having caught up. */
    @Volatile
    private var lastSampleAtMs = 0L

    /**
     * A stale reading is worse than no reading: if the watch stops delivering
     * notifications without dropping the link, the server must learn that the
     * value expired rather than keep showing it.
     */
    private val staleAfterMs = 15_000L

    override fun onCreate() {
        super.onCreate()
        prefs = Prefs(this)
        createChannel()
    }

    override fun onBind(intent: Intent): IBinder? {
        super.onBind(intent)
        return null
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        super.onStartCommand(intent, flags, startId)

        if (intent?.action == ACTION_STOP) {
            PbLog.i(TAG, "service_stop_requested")
            stopEverything()
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }

        if (source != null || startupJob?.isActive == true) return START_STICKY

        // A service launched from a boot receiver must enter the foreground
        // immediately. Bluetooth can still be OFF for a short time during boot,
        // so keep this phase lightweight and initialise the pipeline only once
        // the adapter is ready.
        startForeground(NOTIFICATION_ID, buildNotification("waiting for Bluetooth", null))
        BridgeState.update {
            it.copy(
                running = true,
                startedAtMs = System.currentTimeMillis(),
                sourceStatus = "waiting for Bluetooth",
            )
        }

        startupJob = lifecycleScope.launch {
            while (isActive) {
                if (!prefs.isConfigured()) {
                    PbLog.w(TAG, "configuration_unavailable")
                    stopEverything()
                    stopForeground(STOP_FOREGROUND_REMOVE)
                    stopSelf()
                    return@launch
                }
                if (isBluetoothReady()) {
                    startPipeline()
                    return@launch
                }
                updateStartupStatus("waiting for Bluetooth")
                delay(STARTUP_RETRY_MS)
            }
        }
        return START_STICKY
    }

    private fun isBluetoothReady(): Boolean =
        (getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)
            ?.adapter
            ?.isEnabled == true

    private fun updateStartupStatus(status: String) {
        BridgeState.update { it.copy(sourceStatus = status) }
        getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION_ID, buildNotification(status, null))
    }

    private fun startPipeline() {
        if (source != null) return
        val address = prefs.watchAddress
        val host = prefs.serverHost
        val keyHex = prefs.keyHex
        if (address == null || host.isEmpty() || keyHex.length != 64) {
            stopEverything()
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return
        }

        updateStartupStatus("starting")
        PbLog.i(
            TAG,
            "service_started",
            mapOf(
                "device_id" to prefs.deviceId,
                "source" to prefs.sourceMode,
                "server" to "$host:${prefs.serverPort}",
            ),
        )
        acquireWakeLock()

        sender = UdpSender(
            host = host,
            port = prefs.serverPort,
            deviceId = prefs.deviceId,
            key = PacketCodec.parseKeyHex(keyHex),
            scope = lifecycleScope,
        ).also { it.start() }

        source = createSource(address).also { it.start() }

        // Drives the heartbeat path and the staleness check even when the watch
        // has gone completely silent.
        refreshJob = lifecycleScope.launch {
            while (true) {
                delay(1_000)
                pushSample()
                recoverSilentStreamIfNeeded()
            }
        }

        if (!screenReceiverRegistered) {
            registerReceiver(screenReceiver, IntentFilter(Intent.ACTION_SCREEN_ON))
            screenReceiverRegistered = true
        }
    }

    private fun createSource(address: String): HeartRateSource = when (prefs.sourceMode) {
        SourceMode.MULTILINK -> MultiLinkClient(
            context = this,
            address = address,
            laneIndex = prefs.laneIndex,
            onSample = { hr, resting ->
                if (resting != null) restingHr = resting
                acceptSample(hr, contactOk = true)
            },
            onConnectionChange = ::acceptConnectionChange,
            onStatus = ::acceptSourceStatus,
        )

        SourceMode.BROADCAST -> HrClient(
            context = this,
            address = address,
            // The standard Heart Rate Service carries no resting rate, so
            // whatever Multi-Link last reported is left to expire rather than
            // being presented as current.
            onSample = { hr, contact -> acceptSample(hr, contact) },
            onConnectionChange = ::acceptConnectionChange,
            onStatus = ::acceptSourceStatus,
        )
    }

    private fun acceptSourceStatus(status: String) {
        val now = System.currentTimeMillis()
        PbLog.i(TAG, "source_status", mapOf("status" to status))
        BridgeState.update {
            it.copy(
                sourceStatus = status,
                diagnostics = it.diagnostics.sourceEvent(status, now),
            )
        }
    }

    private fun acceptSample(hr: Int, contactOk: Boolean) {
        lastHr = hr
        this.contactOk = contactOk
        lastSampleAtMs = System.currentTimeMillis()
        streamWatchdog.onSample(SystemClock.elapsedRealtime())
        pushSample()
    }

    private fun acceptConnectionChange(connected: Boolean) {
        watchConnected = connected
        streamWatchdog.onConnectionChanged(connected, SystemClock.elapsedRealtime())
        if (!connected) {
            lastHr = null
            lastSampleAtMs = 0
        }
        pushSample()
    }

    private fun pushSample() {
        val fresh = lastHr != null &&
            lastSampleAtMs != 0L &&
            System.currentTimeMillis() - lastSampleAtMs < staleAfterMs
        val hr = if (fresh) lastHr else null

        sender?.offer(
            UdpSender.Sample(
                heartRate = hr,
                contactOk = contactOk,
                watchConnected = watchConnected,
                batteryPct = batteryPct(),
                restingHr = restingHr ?: 0,
            )
        )

        val client = source
        BridgeState.update {
            it.copy(
                watchConnected = watchConnected,
                heartRate = hr,
                restingHr = restingHr,
                contactOk = contactOk,
                lastSampleAtMs = lastSampleAtMs,
                samples = client?.samples ?: it.samples,
                packetsSent = sender?.packetsSent ?: it.packetsSent,
                reconnects = client?.reconnects ?: it.reconnects,
                lastError = sender?.lastError,
                diagnostics = it.diagnostics.copy(
                    lastUdpSentAtMs = sender?.lastSentAtMs ?: it.diagnostics.lastUdpSentAtMs,
                    udpSendFailures = sender?.sendFailures ?: it.diagnostics.udpSendFailures,
                    lastUdpError = sender?.lastError ?: it.diagnostics.lastUdpError,
                    lastUdpErrorAtMs = sender?.lastErrorAtMs ?: it.diagnostics.lastUdpErrorAtMs,
                ),
            )
        }
        updateNotification(hr)
    }

    private fun recoverSilentStreamIfNeeded() {
        if (!streamWatchdog.shouldRecover(SystemClock.elapsedRealtime())) return
        val reason = "no heart-rate notification for ${STREAM_STALL_AFTER_MS / 1_000}s"
        lastHr = null
        lastSampleAtMs = 0L
        val now = System.currentTimeMillis()
        BridgeState.update {
            it.copy(diagnostics = it.diagnostics.watchdogRecovery(reason, now))
        }
        PbLog.w(TAG, "watchdog_recovery", fields = mapOf("reason" to reason))
        source?.reconnect(reason)
    }

    private fun batteryPct(): Int {
        val bm = getSystemService(Context.BATTERY_SERVICE) as? BatteryManager ?: return 0xFF
        val level = bm.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        return if (level in 0..100) level else 0xFF
    }

    private fun stopEverything() {
        startupJob?.cancel()
        startupJob = null
        refreshJob?.cancel()
        refreshJob = null
        source?.stop()
        source = null
        restingHr = null
        sender?.stop()
        sender = null
        releaseWakeLock()
        if (screenReceiverRegistered) {
            runCatching { unregisterReceiver(screenReceiver) }
            screenReceiverRegistered = false
        }
        BridgeState.reset()
    }

    override fun onDestroy() {
        stopEverything()
        super.onDestroy()
    }

    // Poll immediately on wake instead of waiting for the next one-second tick.
    // The same watchdog also runs while the screen remains off.
    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            pushSample()
            recoverSilentStreamIfNeeded()
        }
    }

    private fun acquireWakeLock() {
        if (wakeLock?.isHeld == true) return
        val power = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = power.newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK,
            "$packageName:BridgeService",
        ).apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    private fun releaseWakeLock() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
    }

    private fun createChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Bridge",
            NotificationManager.IMPORTANCE_LOW,
        ).apply { setShowBadge(false) }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun buildNotification(status: String, hr: Int?): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, BridgeService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val title = if (hr != null) "$hr bpm" else "PulseBridge"
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(title)
            .setContentText(status)
            .setSmallIcon(android.R.drawable.ic_menu_compass)
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(
                Notification.Action.Builder(null, "Stop", stop).build()
            )
            .build()
    }

    private var lastNotifiedHr: Int? = -1
    private var lastNotificationAtMs = 0L
    private var lastNotificationState: Pair<Boolean, Boolean>? = null

    private fun updateNotification(hr: Int?) {
        val now = SystemClock.elapsedRealtime()
        val state = watchConnected to (hr != null)
        val stateChanged = state != lastNotificationState
        val valueChangedAndDue = hr != lastNotifiedHr &&
            now - lastNotificationAtMs >= NOTIFICATION_MIN_INTERVAL_MS
        if (!stateChanged && !valueChangedAndDue) return
        lastNotifiedHr = hr
        lastNotificationAtMs = now
        lastNotificationState = state
        val status = when {
            !watchConnected -> "watch disconnected"
            hr == null -> "connected, waiting for data"
            else -> "streaming to " + prefs.serverHost
        }
        getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION_ID, buildNotification(status, hr))
    }

    companion object {
        const val ACTION_STOP = "me.sagiri.pulsebridge.STOP"
        private const val ACTION_START_AFTER_BOOT = "me.sagiri.pulsebridge.START_AFTER_BOOT"
        private const val TAG = "PulseBridgeService"
        private const val CHANNEL_ID = "bridge"
        private const val NOTIFICATION_ID = 1
        private const val STARTUP_RETRY_MS = 2_000L
        private const val STREAM_STALL_AFTER_MS = 30_000L
        private const val STREAM_RECOVERY_COOLDOWN_MS = 30_000L
        private const val NOTIFICATION_MIN_INTERVAL_MS = 10_000L

        fun start(context: Context) {
            val intent = Intent(context, BridgeService::class.java)
            startForegroundService(context, intent)
        }

        fun startAfterBoot(context: Context) {
            val intent = Intent(context, BridgeService::class.java)
                .setAction(ACTION_START_AFTER_BOOT)
            startForegroundService(context, intent)
        }

        private fun startForegroundService(context: Context, intent: Intent) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        fun stop(context: Context) {
            context.startService(
                Intent(context, BridgeService::class.java).setAction(ACTION_STOP)
            )
        }
    }
}
