package me.sagiri.pulsebridge

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * The one place the UI reads from. Everything downstream of the watch -- the
 * notification, the screen, and later any extra metric -- observes this rather
 * than talking to the BLE or UDP layers directly.
 */
object BridgeState {

    data class Snapshot(
        val running: Boolean = false,
        val watchConnected: Boolean = false,
        val heartRate: Int? = null,
        val contactOk: Boolean = false,
        val lastSampleAtMs: Long = 0,
        val startedAtMs: Long = 0,
        val samples: Long = 0,
        val packetsSent: Long = 0,
        val reconnects: Int = 0,
        val lastError: String? = null,
    ) {
        val uptimeMs: Long
            get() = if (startedAtMs == 0L) 0 else System.currentTimeMillis() - startedAtMs
    }

    private val _state = MutableStateFlow(Snapshot())
    val state: StateFlow<Snapshot> = _state

    fun update(block: (Snapshot) -> Snapshot) {
        _state.value = block(_state.value)
    }

    fun reset() {
        _state.value = Snapshot()
    }
}
