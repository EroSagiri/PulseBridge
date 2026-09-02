package me.sagiri.pulsebridge.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.Build
import android.os.IBinder
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import me.sagiri.pulsebridge.BridgeState
import me.sagiri.pulsebridge.HeartRateSource
import me.sagiri.pulsebridge.MainActivity
import me.sagiri.pulsebridge.Prefs
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
    private var refreshJob: Job? = null

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
            stopEverything()
            stopSelf()
            return START_NOT_STICKY
        }

        if (source != null) return START_STICKY

        val address = prefs.watchAddress
        val host = prefs.serverHost
        val keyHex = prefs.keyHex
        if (address == null || host.isEmpty() || keyHex.length != 64) {
            stopSelf()
            return START_NOT_STICKY
        }

        startForeground(NOTIFICATION_ID, buildNotification("starting", null))

        BridgeState.update {
            it.copy(running = true, startedAtMs = System.currentTimeMillis())
        }

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
            }
        }

        registerReceiver(screenReceiver, IntentFilter(Intent.ACTION_SCREEN_ON))
        return START_STICKY
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
            onStatus = { status ->
                BridgeState.update { it.copy(sourceStatus = status) }
            },
        )

        SourceMode.BROADCAST -> HrClient(
            context = this,
            address = address,
            // The standard Heart Rate Service carries no resting rate, so
            // whatever Multi-Link last reported is left to expire rather than
            // being presented as current.
            onSample = { hr, contact -> acceptSample(hr, contact) },
            onConnectionChange = ::acceptConnectionChange,
        )
    }

    private fun acceptSample(hr: Int, contactOk: Boolean) {
        lastHr = hr
        this.contactOk = contactOk
        lastSampleAtMs = System.currentTimeMillis()
        pushSample()
    }

    private fun acceptConnectionChange(connected: Boolean) {
        watchConnected = connected
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
            )
        }
        updateNotification(hr)
    }

    private fun batteryPct(): Int {
        val bm = getSystemService(Context.BATTERY_SERVICE) as? BatteryManager ?: return 0xFF
        val level = bm.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        return if (level in 0..100) level else 0xFF
    }

    private fun stopEverything() {
        refreshJob?.cancel()
        refreshJob = null
        source?.stop()
        source = null
        restingHr = null
        sender?.stop()
        sender = null
        runCatching { unregisterReceiver(screenReceiver) }
        BridgeState.reset()
    }

    override fun onDestroy() {
        stopEverything()
        super.onDestroy()
    }

    // Some OEM builds suspend BLE callbacks while the screen is off and only
    // resume on wake; nudging the state machine here surfaces that as a
    // reconnect instead of a silent stall.
    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) = pushSample()
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

    private fun updateNotification(hr: Int?) {
        // Rewriting the notification at 1 Hz is a measurable battery cost of
        // its own, so only touch it when the displayed value actually changes.
        if (hr == lastNotifiedHr) return
        lastNotifiedHr = hr
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
        private const val CHANNEL_ID = "bridge"
        private const val NOTIFICATION_ID = 1

        fun start(context: Context) {
            val intent = Intent(context, BridgeService::class.java)
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
