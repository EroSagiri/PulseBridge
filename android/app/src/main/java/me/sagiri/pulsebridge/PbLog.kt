package me.sagiri.pulsebridge

import android.util.Log

/** Small native Logcat wrapper with stable event and field formatting. */
object PbLog {
    fun d(tag: String, event: String, fields: Map<String, Any?> = emptyMap()) {
        Log.d(tag, format(event, fields))
    }

    fun i(tag: String, event: String, fields: Map<String, Any?> = emptyMap()) {
        Log.i(tag, format(event, fields))
    }

    fun e(
        tag: String,
        event: String,
        error: Throwable? = null,
        fields: Map<String, Any?> = emptyMap(),
    ) {
        Log.e(tag, format(event, fields), error)
    }

    fun w(
        tag: String,
        event: String,
        error: Throwable? = null,
        fields: Map<String, Any?> = emptyMap(),
    ) {
        Log.w(tag, format(event, fields), error)
    }

    private fun format(event: String, fields: Map<String, Any?>): String = buildString {
        append(event)
        fields.forEach { (key, value) ->
            if (value != null) append(' ').append(key).append('=').append(value)
        }
    }
}
