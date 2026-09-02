package me.sagiri.pulsebridge

/**
 * A live heart-rate feed. The bridge service does not care whether the samples
 * came off the standard Heart Rate Service or off Garmin's private Multi-Link
 * channel, which is what lets the two be swapped at runtime.
 */
interface HeartRateSource {
    fun start()
    fun stop()

    /** Link drops since start; the headline number for overnight stability. */
    val reconnects: Int

    /** Samples accepted from the watch, for comparing against uptime. */
    val samples: Long
}

enum class SourceMode {
    /** Standard BLE Heart Rate Service. Needs broadcast mode on the watch. */
    BROADCAST,

    /** Garmin private Multi-Link. Coexists with Garmin Connect, no watch setup. */
    MULTILINK;

    companion object {
        fun from(name: String?): SourceMode =
            entries.firstOrNull { it.name == name } ?: MULTILINK
    }
}
