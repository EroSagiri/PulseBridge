package me.sagiri.pulsebridge

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import me.sagiri.pulsebridge.ble.HrScanner
import me.sagiri.pulsebridge.service.BridgeService
import java.security.SecureRandom
import java.util.Locale

class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme { Screen(Prefs(this)) }
        }
    }
}

private fun requiredPermissions(): Array<String> =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
    } else {
        arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
    }

@SuppressLint("BatteryLife")
@Composable
private fun Screen(prefs: Prefs) {
    val state by BridgeState.state.collectAsStateWithLifecycle()

    var host by remember { mutableStateOf(prefs.serverHost) }
    var port by remember { mutableStateOf(prefs.serverPort.toString()) }
    var keyHex by remember { mutableStateOf(prefs.keyHex) }
    var watchName by remember { mutableStateOf(prefs.watchName ?: "") }
    var watchAddress by remember { mutableStateOf(prefs.watchAddress ?: "") }
    var autoStart by remember { mutableStateOf(prefs.autoStart) }
    var sourceMode by remember { mutableStateOf(prefs.sourceMode) }
    var lane by remember { mutableStateOf(prefs.laneIndex.toString()) }
    var scanning by remember { mutableStateOf(false) }
    val found = remember { mutableStateListOf<Pair<String, String>>() }

    val context = androidx.compose.ui.platform.LocalContext.current
    val scanner = remember {
        HrScanner(context) { address, name ->
            if (found.none { it.first == address }) found.add(address to name)
        }
    }
    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) { granted ->
        if (granted.values.all { it }) {
            found.clear()
            scanning = scanner.start()
        }
    }
    val notifLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { }

    LaunchedEffect(Unit) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            notifLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("PulseBridge", fontSize = 26.sp, fontWeight = FontWeight.Bold)

        // ---- live status -------------------------------------------------
        Card(Modifier.fillMaxWidth()) {
            Column(
                Modifier.padding(20.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(
                    text = state.heartRate?.toString() ?: "--",
                    fontSize = 64.sp,
                    fontWeight = FontWeight.Bold,
                )
                Text("BPM", fontSize = 13.sp, letterSpacing = 2.sp)
                Spacer(Modifier.height(14.dp))
                val status = when {
                    !state.running -> "stopped"
                    !state.watchConnected -> "waiting for watch"
                    state.heartRate == null -> "connected, no data"
                    else -> "streaming"
                }
                Text(status)
                Spacer(Modifier.height(10.dp))
                // These counters are what the broadcast battery experiment is
                // read against: note the watch battery at start and at end,
                // and divide by uptime.
                StatRow("uptime", formatDuration(state.uptimeMs))
                StatRow("samples from watch", state.samples.toString())
                StatRow("packets sent", state.packetsSent.toString())
                StatRow("ble reconnects", state.reconnects.toString())
                StatRow("sensor contact", if (state.contactOk) "ok" else "poor")
                state.restingHr?.let { StatRow("resting hr", "$it bpm") }
                state.sourceStatus?.let { StatRow("source", it) }
                state.lastError?.let { StatRow("last error", it) }
            }
        }

        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(
                enabled = host.isNotBlank() && keyHex.length == 64 && watchAddress.isNotBlank(),
                onClick = {
                    prefs.serverHost = host
                    prefs.serverPort = port.toIntOrNull() ?: 9999
                    prefs.keyHex = keyHex
                    prefs.watchAddress = watchAddress
                    prefs.watchName = watchName
                    prefs.sourceMode = sourceMode
                    prefs.laneIndex = lane.toIntOrNull() ?: 0
                    scanner.stop()
                    scanning = false
                    BridgeService.start(context)
                },
            ) { Text(if (state.running) "Restart" else "Start") }

            OutlinedButton(
                enabled = state.running,
                onClick = { BridgeService.stop(context) },
            ) { Text("Stop") }
        }

        // ---- source ------------------------------------------------------
        Text("Source", fontWeight = FontWeight.SemiBold)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(
                selected = sourceMode == SourceMode.MULTILINK,
                onClick = { sourceMode = SourceMode.MULTILINK },
                label = { Text("Multi-Link") },
            )
            FilterChip(
                selected = sourceMode == SourceMode.BROADCAST,
                onClick = { sourceMode = SourceMode.BROADCAST },
                label = { Text("Broadcast") },
            )
        }
        Text(
            when (sourceMode) {
                SourceMode.MULTILINK ->
                    "The private Garmin channel. Nothing to switch on at the watch, and " +
                        "Garmin Connect keeps running. Also carries resting heart rate."

                SourceMode.BROADCAST ->
                    "Standard Heart Rate Service. Switch on Broadcast Heart Rate at the " +
                        "watch first: hold UP, Health and Wellness, Wrist Heart Rate, " +
                        "Broadcast Heart Rate."
            },
            fontSize = 13.sp,
        )

        if (sourceMode == SourceMode.MULTILINK) {
            OutlinedTextField(
                value = lane,
                onValueChange = { lane = it.filter { c -> c.isDigit() }.take(1) },
                label = { Text("Multi-Link lane") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Text(
                "Lane 0 by default. Garmin Connect holds lane 1 on a Forerunner 255, so " +
                    "only move off it if registration reports the lane is already in use.",
                fontSize = 12.sp,
            )
        }

        // ---- pairing -----------------------------------------------------
        Text("Watch", fontWeight = FontWeight.SemiBold)
        if (watchAddress.isNotBlank()) {
            Text("selected: $watchName  ($watchAddress)", fontSize = 13.sp)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = {
                if (scanning) {
                    scanner.stop()
                    scanning = false
                } else {
                    found.clear()
                    // Multi-Link runs over the existing bond, so the watch is
                    // already in the paired list and never advertises 0x180D
                    // unless broadcast mode happens to be on as well.
                    if (sourceMode == SourceMode.MULTILINK) {
                        found.addAll(bondedDevices(context))
                    } else {
                        permissionLauncher.launch(requiredPermissions())
                    }
                }
            }) {
                Text(
                    when {
                        scanning -> "Stop scan"
                        sourceMode == SourceMode.MULTILINK -> "List paired devices"
                        else -> "Scan"
                    }
                )
            }
        }
        found.forEach { (address, name) ->
            OutlinedButton(
                modifier = Modifier.fillMaxWidth(),
                onClick = {
                    watchAddress = address
                    watchName = name
                    prefs.watchAddress = address
                    prefs.watchName = name
                    scanner.stop()
                    scanning = false
                },
            ) { Text("$name   $address") }
        }

        // ---- server ------------------------------------------------------
        Text("Server", fontWeight = FontWeight.SemiBold)
        OutlinedTextField(
            value = host,
            onValueChange = { host = it },
            label = { Text("host or IP") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = port,
            onValueChange = { port = it.filter { c -> c.isDigit() } },
            label = { Text("UDP port") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = keyHex,
            onValueChange = { keyHex = it.trim().lowercase(Locale.ROOT) },
            label = { Text("shared key (64 hex chars)") },
            singleLine = true,
            isError = keyHex.isNotEmpty() && keyHex.length != 64,
            textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
            modifier = Modifier.fillMaxWidth(),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            OutlinedButton(onClick = {
                val bytes = ByteArray(32).also { SecureRandom().nextBytes(it) }
                keyHex = bytes.joinToString("") { b -> "%02x".format(b) }
            }) { Text("Generate key") }
        }
        Text("device id: ${prefs.deviceId}", fontSize = 13.sp)

        // ---- power -------------------------------------------------------
        Text("Background", fontWeight = FontWeight.SemiBold)
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("start after reboot")
            Switch(checked = autoStart, onCheckedChange = {
                autoStart = it
                prefs.autoStart = it
            })
        }
        if (!isIgnoringBatteryOptimizations(context)) {
            OutlinedButton(onClick = {
                context.startActivity(
                    Intent(
                        Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
                        Uri.parse("package:" + context.packageName),
                    )
                )
            }) { Text("Exempt from battery optimisation") }
            Text(
                "Without this, the system will eventually suspend the BLE link " +
                    "and the stream stops while the screen is off.",
                fontSize = 12.sp,
            )
        } else {
            Text("battery optimisation: exempt", fontSize = 13.sp)
        }
    }
}

@Composable
private fun StatRow(label: String, value: String) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(vertical = 2.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(label, fontSize = 13.sp)
        Text(value, fontSize = 13.sp, fontFamily = FontFamily.Monospace)
    }
}

@SuppressLint("MissingPermission")
private fun bondedDevices(context: Context): List<Pair<String, String>> {
    val adapter = (context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter
    return adapter?.bondedDevices.orEmpty()
        .map { it.address to (it.name ?: "(unnamed)") }
        .sortedBy { it.second }
}

private fun isIgnoringBatteryOptimizations(context: Context): Boolean {
    val pm = context.getSystemService(Context.POWER_SERVICE) as? PowerManager ?: return false
    return pm.isIgnoringBatteryOptimizations(context.packageName)
}

private fun formatDuration(ms: Long): String {
    val total = ms / 1000
    return "%d:%02d:%02d".format(total / 3600, (total % 3600) / 60, total % 60)
}
