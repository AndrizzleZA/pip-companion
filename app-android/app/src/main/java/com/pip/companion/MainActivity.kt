package com.pip.companion

import android.Manifest
import android.app.AlarmManager
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : ComponentActivity() {
    private lateinit var store: Store
    private lateinit var sync: Sync
    private var verifier: String? = null

    private val ui = mutableStateOf(Screen())

    data class Screen(
        val connected: Boolean = false,
        val waiting: Boolean = false,
        val refused: Boolean = false,
        val busy: Boolean = false,
        val note: String? = null,
        val reminders: List<Reminder> = emptyList(),
        val asking: List<JSONObject> = emptyList(),
        /// Pip can't actually alert anyone — notifications are off, or alarms can't be exact.
        val silenced: String? = null,
    )

    data class Reminder(val id: String, val title: String, val times: String, val enabled: Boolean)

    private val askNotifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) { refresh() }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        store = Store(this)
        sync = Sync(store, deviceName())
        Alarms.channel(this)
        refresh()
        setContent { PipTheme { Home(ui.value, ::connect, ::syncNow, ::approve, ::deny, ::askAgain, ::toggle, ::fixAlerts) } }
        if (store.refreshToken != null) syncNow()
        // Whatever happens, make sure the next reminder is armed — a reminder that doesn't go off
        // is the whole app failing.
        Alarms.arm(this, store)
        handle(intent)
    }

    override fun onResume() {
        super.onResume()
        refresh() // permissions may have been changed while Pip was in the background
        Alarms.arm(this, store)
    }

    /// Takes the user to whichever switch is stopping reminders from arriving.
    private fun fixAlerts() {
        if (!NotificationManagerCompat.from(this).areNotificationsEnabled()) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
            ) {
                askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            } else {
                startActivity(
                    Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                        .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, packageName),
                )
            }
            return
        }
        startActivity(exactAlarmSettings(this))
    }

    /// What, if anything, is stopping a reminder from reaching you.
    private fun silencedReason(): String? {
        if (!NotificationManagerCompat.from(this).areNotificationsEnabled()) {
            return "Pip can't show reminders until notifications are allowed."
        }
        val am = getSystemService(AlarmManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && !am.canScheduleExactAlarms()) {
            return "Reminders may arrive late until Pip is allowed to set alarms."
        }
        return null
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handle(intent)
    }

    /** Google handing the sign-in back. */
    private fun handle(intent: Intent?) {
        val data = intent?.data ?: return
        if (data.scheme != "com.pip.companion") return
        val error = data.getQueryParameter("error")
        val code = data.getQueryParameter("code")
        val v = verifier
        when {
            error != null -> ui.value = ui.value.copy(note = "Sign-in was cancelled. Nothing changed.")
            code != null && v != null -> work("Connecting…") {
                val (refresh, _) = Auth.exchange(code, v)
                store.refreshToken = refresh
                verifier = null
                sync.round()
            }
        }
    }

    private fun connect() {
        if (BuildConfig.GOOGLE_CLIENT_ID.isEmpty()) {
            ui.value = ui.value.copy(note = "This build has no sign-in identity yet.")
            return
        }
        val v = Auth.randomUrlSafe(48)
        verifier = v
        CustomTabsIntent.Builder().build().launchUrl(this, android.net.Uri.parse(Auth.consentUrl(v, Auth.randomUrlSafe(12))))
    }

    private fun syncNow() = work("Syncing…") { sync.round() }

    private fun approve(request: JSONObject) = work("Letting it in…") { sync.round(approve = request) }

    private fun deny(request: JSONObject) = work("Refusing…") { sync.round(deny = listOf(request.getString("id"))) }

    private fun askAgain() {
        sync.askAgain()
        syncNow()
    }

    /** Turning a reminder on or off is a normal edit; the next round carries it everywhere. */
    private fun toggle(id: String) {
        val list = store.reminders()
        val now = Auth.now()
        val next = JSONArray()
        for (i in 0 until list.length()) {
            val r = list.getJSONObject(i)
            if (r.optString("id") == id) {
                r.put("enabled", !r.optBoolean("enabled", true))
                r.put("updated_at", now)
            }
            next.put(r)
        }
        store.putReminders(next, now)
        refresh()
        Alarms.arm(this, store) // the next thing due may have just changed
        syncNow()
    }

    private fun work(busyNote: String, block: () -> Sync.Outcome) {
        ui.value = ui.value.copy(busy = true, note = busyNote)
        lifecycleScope.launch {
            val outcome = withContext(Dispatchers.IO) { block() }
            if (outcome.signedOut) store.forgetSync()
            refresh(outcome)
            // A sync can bring in new reminders or new times from the PC.
            Alarms.arm(this@MainActivity, store)
        }
    }

    private fun refresh(outcome: Sync.Outcome? = null) {
        val list = store.reminders()
        val reminders = (0 until list.length()).map { list.getJSONObject(it) }
            .sortedBy { it.optInt("time") }
            .map { r ->
                val list = r.optJSONArray("times")
                val times = when {
                    list != null && list.length() > 0 -> (0 until list.length()).map { list.getString(it) }
                    else -> listOf(r.optString("time", "09:00"))
                }
                Reminder(
                    id = r.optString("id"),
                    title = r.optString("title").ifEmpty { "Reminder" },
                    times = times.joinToString(", "),
                    enabled = r.optBoolean("enabled", true),
                )
            }
        ui.value = Screen(
            connected = store.refreshToken != null && store.syncKey != null,
            waiting = outcome?.waiting ?: (store.refreshToken != null && store.syncKey == null && !store.refused),
            refused = outcome?.refused ?: store.refused,
            busy = false,
            note = outcome?.error ?: noteFor(outcome),
            reminders = reminders,
            asking = outcome?.asking ?: emptyList(),
            silenced = silencedReason(),
        )
    }

    private fun noteFor(outcome: Sync.Outcome?): String? = when {
        outcome == null -> null
        outcome.lostRace -> "Your PC set sync up first, so Pip asked it to let this phone in."
        outcome.waiting -> "Open Pip on your PC and tap Approve. Nothing to type."
        outcome.refused -> "Your PC said not now, so this phone isn't syncing yet."
        outcome.synced -> "Up to date."
        else -> null
    }

    private fun deviceName(): String {
        val model = "${Build.MANUFACTURER} ${Build.MODEL}".trim()
        return model.replaceFirstChar { it.uppercase() }.ifEmpty { "Android phone" }
    }
}

@Composable
private fun Home(
    s: MainActivity.Screen,
    connect: () -> Unit,
    syncNow: () -> Unit,
    approve: (JSONObject) -> Unit,
    deny: (JSONObject) -> Unit,
    askAgain: () -> Unit,
    toggle: (String) -> Unit,
    fixAlerts: () -> Unit,
) {
    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().padding(20.dp).verticalScroll(rememberScrollState())) {
            Text("Pip", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.height(4.dp))
            Text(
                if (s.connected) "Your reminders, the same here as on your PC." else "Sign in with the same Google account as your PC.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(16.dp))

            if (!s.connected) {
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(16.dp)) {
                        when {
                            s.refused -> {
                                Text("Not approved", fontWeight = FontWeight.SemiBold)
                                Text("Your PC said not now. You can ask it again.", style = MaterialTheme.typography.bodySmall)
                                Spacer(Modifier.height(10.dp))
                                Button(onClick = askAgain, enabled = !s.busy) { Text("Ask again") }
                            }
                            s.waiting -> {
                                Text("Waiting for your PC", fontWeight = FontWeight.SemiBold)
                                Text("Open Pip there and tap Approve. Nothing to type.", style = MaterialTheme.typography.bodySmall)
                                Spacer(Modifier.height(10.dp))
                                OutlinedButton(onClick = syncNow, enabled = !s.busy) { Text("Check again") }
                            }
                            else -> {
                                Text("Connect this phone", fontWeight = FontWeight.SemiBold)
                                Text("Pip only gets its own hidden folder in your Drive — nothing else.", style = MaterialTheme.typography.bodySmall)
                                Spacer(Modifier.height(10.dp))
                                Button(onClick = connect, enabled = !s.busy) { Text("Continue with Google") }
                            }
                        }
                    }
                }
                Spacer(Modifier.height(16.dp))
            }

            for (request in s.asking) {
                Card(Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column(Modifier.padding(16.dp)) {
                        Text("${request.optString("device")} wants to sync", fontWeight = FontWeight.SemiBold)
                        Text("Approve only if you just signed in there.", style = MaterialTheme.typography.bodySmall)
                        Spacer(Modifier.height(10.dp))
                        Row {
                            Button(onClick = { approve(request) }, enabled = !s.busy) { Text("Approve") }
                            Spacer(Modifier.width(8.dp))
                            TextButton(onClick = { deny(request) }, enabled = !s.busy) { Text("Not now") }
                        }
                    }
                }
            }

            // Say it plainly when a reminder wouldn't actually reach them, and offer the fix.
            s.silenced?.let { why ->
                Card(Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column(Modifier.padding(16.dp)) {
                        Text("Reminders are muted", fontWeight = FontWeight.SemiBold)
                        Text(why, style = MaterialTheme.typography.bodySmall)
                        Spacer(Modifier.height(10.dp))
                        Button(onClick = fixAlerts) { Text("Fix this") }
                    }
                }
            }

            if (s.connected) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Reminders", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                    TextButton(onClick = syncNow, enabled = !s.busy) { Text("Sync now") }
                }
                if (s.reminders.isEmpty()) {
                    Text("Nothing yet. Add one on your PC and it will appear here.", style = MaterialTheme.typography.bodySmall)
                }
                for (r in s.reminders) {
                    ListItem(
                        headlineContent = { Text(r.title) },
                        supportingContent = { Text(r.times) },
                        trailingContent = { Switch(checked = r.enabled, onCheckedChange = { toggle(r.id) }, enabled = !s.busy) },
                    )
                    HorizontalDivider()
                }
            }

            s.note?.let {
                Spacer(Modifier.height(16.dp))
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

@Composable
private fun PipTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme(), content = content)
}
