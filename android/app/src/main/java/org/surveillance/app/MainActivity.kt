package org.surveillance.app

import android.Manifest
import android.annotation.SuppressLint
import android.content.pm.PackageManager
import android.app.Application
import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.viewModelScope
import java.io.IOException
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

class MainActivity : ComponentActivity() {
    private val model: MonitorModel by viewModels()
    // This ComponentActivity uses no fragments.
    @SuppressLint("InvalidFragmentVersionForActivityResult")
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        model.checkNotifications()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        model.openEvent(eventId(intent.getStringExtra(eventExtra)))
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                model.checkNotifications()
                while (isActive) {
                    model.refresh()
                    delay(7_000)
                }
            }
        }
        setContent {
            val state by model.state.collectAsStateWithLifecycle()
            val colors = if (isSystemInDarkTheme()) {
                darkColorScheme(primary = Color(0xFF8FD6B5))
            } else {
                lightColorScheme(primary = Color(0xFF22664D))
            }
            MaterialTheme(colorScheme = colors) {
                MonitorScreen(state, model, ::enableNotifications)
            }
        }
    }

    private fun enableNotifications() {
        val settings = getPreferences(MODE_PRIVATE)
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED &&
            (!settings.getBoolean("asked_for_notifications", false) || shouldShowRequestPermissionRationale(Manifest.permission.POST_NOTIFICATIONS))
        ) {
            settings.edit().putBoolean("asked_for_notifications", true).apply()
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            startActivity(Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, packageName))
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        model.openEvent(eventId(intent.getStringExtra(eventExtra)))
    }
}

data class MonitorState(
    val origin: String? = null,
    val status: HostStatus? = null,
    val events: List<Event> = emptyList(),
    val nextBefore: Long? = null,
    val detailId: String? = null,
    val detail: Event? = null,
    val detailMissing: Boolean = false,
    val loading: Boolean = false,
    val busy: Boolean = false,
    val notificationsAllowed: Boolean = false,
    val firebaseAvailable: Boolean = false,
    val error: String? = null,
)

class MonitorModel(private val app: Application) : AndroidViewModel(app) {
    private val store = ConnectionStore(app)
    private var connection: Connection? = null
    private var firstPage = emptyList<Event>()
    private val mutableState = MutableStateFlow(MonitorState(loading = true))
    val state = mutableState.asStateFlow()

    init {
        viewModelScope.launch {
            try {
                connection = io { store.load() }
                mutableState.update { it.copy(origin = connection?.origin) }
            } catch (_: java.security.GeneralSecurityException) {
                io { store.clear() }
                mutableState.update { it.copy(error = "Saved credentials are unreadable. Revoke the old pairing on the host, then pair again.") }
            } finally {
                mutableState.update { it.copy(loading = false) }
            }
            checkNotifications()
            refresh()
        }
    }

    fun checkNotifications() {
        mutableState.update {
            it.copy(notificationsAllowed = notificationsAllowed(app), firebaseAvailable = firebaseAvailable(app))
        }
        if (connection != null) schedulePushRegistration(app)
    }

    fun pair(address: String, code: String) = action {
        val origin = hostOrigin(address)
        val credential = code.trim()
        require(credential.length == 43) { "Enter the pairing code printed by the host" }
        val paired = io {
            HostApi(origin).pair(credential).also { store.save(it) }
        }
        connection = paired
        mutableState.update { it.copy(origin = paired.origin) }
        checkNotifications()
        load()
    }

    fun refresh() {
        if (connection == null) return
        action {
            try {
                load()
            } catch (error: IOException) {
                mutableState.update { it.copy(status = null) }
                throw error
            }
        }
    }

    fun arm(value: Boolean) = action {
        val current = connection ?: return@action
        io { HostApi(current).setArmed(value) }
        load()
    }

    fun loadMore() = action {
        val current = connection ?: return@action
        val before = mutableState.value.nextBefore ?: return@action
        val page = io { HostApi(current).events(before) }
        mutableState.update {
            it.copy(events = (it.events + page.events).distinctBy(Event::id), nextBefore = page.nextBefore)
        }
    }

    fun disconnect() = action {
        val current = connection ?: return@action
        io {
            try {
                HostApi(current).disconnect()
            } catch (error: HttpFailure) {
                if (error.status != 401) throw error
            }
            store.clear()
        }
        connection = null
        firstPage = emptyList()
        mutableState.update { MonitorState(notificationsAllowed = it.notificationsAllowed, firebaseAvailable = it.firebaseAvailable) }
    }

    fun openEvent(id: String?) {
        if (id == null) return
        mutableState.update { it.copy(detailId = id, detail = it.events.find { event -> event.id == id }, detailMissing = false) }
        refresh()
    }

    fun closeEvent() {
        mutableState.update { it.copy(detailId = null, detail = null, detailMissing = false) }
    }

    private suspend fun load() {
        val current = connection ?: return
        val api = HostApi(current)
        val (status, page) = io { api.status() to api.events() }
        mutableState.update {
            if (page.events == firstPage) it.copy(status = status)
            else it.copy(status = status, events = page.events, nextBefore = page.nextBefore)
        }
        firstPage = page.events
        val snapshot = mutableState.value
        val id = snapshot.detailId ?: return
        if (snapshot.detail != null || snapshot.detailMissing) return
        try {
            val event = io { api.event(id) }
            mutableState.update { if (it.detailId == id) it.copy(detail = event) else it }
        } catch (error: HttpFailure) {
            if (error.status != 404) throw error
            mutableState.update { if (it.detailId == id) it.copy(detailMissing = true) else it }
        }
    }

    private fun action(operation: suspend () -> Unit) {
        if (mutableState.value.busy || mutableState.value.loading) return
        mutableState.update { it.copy(busy = true) }
        viewModelScope.launch {
            try {
                operation()
                mutableState.update { it.copy(error = null) }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                val message = when (error) {
                    is HttpFailure -> when (error.status) {
                        401 -> if (connection == null) "The pairing code is invalid or expired." else "Pairing was revoked. Disconnect this phone to pair again."
                        else -> "The host returned HTTP ${error.status}."
                    }
                    is IOException -> "Cannot reach the host. Check its address and your Tailscale connection."
                    is IllegalArgumentException -> error.message ?: "Check the connection details."
                    else -> "Could not complete the operation."
                }
                mutableState.update { it.copy(error = message) }
            } finally {
                mutableState.update { it.copy(busy = false) }
            }
        }
    }

    private suspend fun <T> io(action: () -> T): T = withContext(Dispatchers.IO) { action() }
}

@Composable
private fun MonitorScreen(state: MonitorState, model: MonitorModel, enableNotifications: () -> Unit) {
    var origin by rememberSaveable { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    Scaffold { padding ->
        LazyColumn(
            modifier = Modifier.padding(padding).padding(horizontal = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item { Text("Surveillance", style = MaterialTheme.typography.headlineMedium, modifier = Modifier.padding(top = 16.dp)) }
            state.error?.let { message ->
                item { Text(message, color = MaterialTheme.colorScheme.error) }
            }
            if (state.loading) {
                item { Text("Loading connection…") }
            } else if (state.origin == null) {
                item { Text("Pair with a host", style = MaterialTheme.typography.titleLarge) }
                item {
                    OutlinedTextField(
                        origin, { origin = it }, label = { Text("Host HTTPS address") },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                        modifier = Modifier.fillMaxWidth(), singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        code, { code = it }, label = { Text("One-time pairing code") },
                        visualTransformation = PasswordVisualTransformation(),
                        modifier = Modifier.fillMaxWidth(), singleLine = true,
                    )
                }
                item {
                    Button(onClick = { model.pair(origin, code) }, enabled = !state.busy && origin.isNotBlank() && code.isNotBlank()) {
                        Text("Pair phone")
                    }
                }
            } else {
                item {
                    Card(Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            val status = state.status
                            Text(
                                when (status?.armed) { true -> "Armed"; false -> "Disarmed"; null -> "Host unavailable" },
                                style = MaterialTheme.typography.titleLarge,
                            )
                            Text(state.origin, style = MaterialTheme.typography.bodySmall)
                            if (status != null) {
                                Text("Camera · ${sensorLabel(status.camera)}")
                                Text("Microphone · ${sensorLabel(status.microphone)}")
                                Button(onClick = { model.arm(!status.armed) }, enabled = !state.busy) {
                                    Text(if (status.armed) "Disarm" else "Arm")
                                }
                            }
                        }
                    }
                }
                item {
                    when {
                        !state.firebaseAvailable -> Text("This build has no Firebase configuration.")
                        !state.notificationsAllowed -> TextButton(onClick = enableNotifications) { Text("Enable notifications") }
                        state.status?.pushConfigured == false -> Text("Configure Firebase on the host to receive alerts.")
                        state.status?.pushRegistered == false -> Text("Registering this phone for alerts…")
                    }
                }
                item {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text("Events", style = MaterialTheme.typography.titleLarge)
                        TextButton(onClick = model::refresh, enabled = !state.busy) { Text("Refresh") }
                    }
                }
                if (state.events.isEmpty()) item { Text("No events to display.") }
                items(state.events, key = Event::id) { event ->
                    Card(onClick = { model.openEvent(event.id) }, modifier = Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(16.dp)) {
                            Text(eventLabel(event.kind), style = MaterialTheme.typography.titleMedium)
                            Text(eventTime(event), style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
                if (state.nextBefore != null) {
                    item { TextButton(onClick = model::loadMore, enabled = !state.busy) { Text("Older events") } }
                }
                item { TextButton(onClick = model::disconnect, enabled = !state.busy) { Text("Disconnect phone") } }
            }
        }
    }
    if (state.origin != null && state.detailId != null) {
        AlertDialog(
            onDismissRequest = model::closeEvent,
            title = { Text(state.detail?.let { eventLabel(it.kind) } ?: "Event") },
            text = {
                Text(
                    state.detail?.let(::eventTime) ?: when {
                        state.detailMissing -> "This event has expired."
                        state.error != null -> state.error
                        else -> "Loading event…"
                    },
                )
            },
            confirmButton = { TextButton(onClick = model::closeEvent) { Text("Close") } },
        )
    }
}

private fun sensorLabel(state: String): String = when (state) {
    "disabled" -> "Disabled"
    "idle" -> "Idle"
    "starting" -> "Starting"
    "ready" -> "Ready"
    "permission_needed" -> "Allow access on the Mac"
    "permission_denied" -> "Permission denied"
    else -> "Unavailable"
}

private fun eventLabel(kind: String): String = when (kind) {
    "motion" -> "Motion detected"
    "sound" -> "Sound detected"
    "test" -> "Test alert"
    else -> "Surveillance event"
}

private fun eventTime(event: Event): String = DateFormat.getDateTimeInstance().format(Date(event.occurredAtMs))
