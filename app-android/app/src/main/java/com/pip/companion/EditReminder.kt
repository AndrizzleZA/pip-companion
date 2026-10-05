package com.pip.companion

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import org.json.JSONArray
import org.json.JSONObject

/**
 * One reminder, open for editing.
 *
 * Deliberately one screen with no sub-pages: a name, the times it goes off, the days it repeats,
 * and whether it may break through quiet hours. Times are chips you tap to change and can add to,
 * which is how medication apps handle "morning and night" without turning one thing into three.
 */
data class Draft(
    val id: String,
    var title: String,
    var times: List<String>,
    var days: Int,
    var important: Boolean,
    var enabled: Boolean,
    val isNew: Boolean,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("id", id)
        .put("title", title.ifBlank { "Reminder" })
        .put("times", JSONArray(times))
        .put("days", days)
        .put("important", important)
        .put("enabled", enabled)

    companion object {
        fun of(r: JSONObject): Draft {
            val list = r.optJSONArray("times")
            val times = when {
                list != null && list.length() > 0 -> (0 until list.length()).map { list.getString(it) }
                else -> listOf(r.optString("time", "09:00"))
            }
            return Draft(
                id = r.optString("id"),
                title = r.optString("title"),
                times = times,
                days = r.optInt("days", WEEKDAYS),
                important = r.optBoolean("important", false),
                enabled = r.optBoolean("enabled", true),
                isNew = false,
            )
        }

        fun blank(id: String) = Draft(id, "", listOf("09:00"), WEEKDAYS, important = false, enabled = true, isNew = true)

        const val WEEKDAYS = 0b001_1111
        const val EVERY_DAY = 0b111_1111
        /** Bit 0 is Monday, matching the engine. */
        val DAY_LETTERS = listOf("M", "T", "W", "T", "F", "S", "S")
    }
}

/** "Weekdays", "Every day", "Mon, Wed, Fri" — or "Once" when nothing repeats. */
fun describeDays(days: Int): String = when {
    days == 0 -> "Once"
    days == Draft.EVERY_DAY -> "Every day"
    days == Draft.WEEKDAYS -> "Weekdays"
    days == 0b110_0000 -> "Weekends"
    else -> {
        val names = listOf("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")
        (0..6).filter { days and (1 shl it) != 0 }.joinToString(", ") { names[it] }
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun EditReminder(draft: Draft, onSave: (Draft) -> Unit, onDelete: () -> Unit, onCancel: () -> Unit) {
    var title by remember { mutableStateOf(draft.title) }
    var times by remember { mutableStateOf(draft.times) }
    var days by remember { mutableStateOf(draft.days) }
    var important by remember { mutableStateOf(draft.important) }
    var picking by remember { mutableStateOf<Int?>(null) } // index being set, or times.size for a new one
    var confirmDelete by remember { mutableStateOf(false) }

    Dialog(onDismissRequest = onCancel, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            Column(Modifier.fillMaxSize()) {
                TopAppBar(
                    title = { Text(if (draft.isNew) "New reminder" else "Reminder") },
                    navigationIcon = { TextButton(onClick = onCancel) { Text("Cancel") } },
                    actions = {
                        TextButton(onClick = {
                            onSave(draft.copy(title = title, times = times.sorted(), days = days, important = important))
                        }) { Text("Save", fontWeight = FontWeight.SemiBold) }
                    },
                )
                Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(20.dp)) {
                    OutlinedTextField(
                        value = title,
                        onValueChange = { title = it.take(60) },
                        label = { Text("What is it?") },
                        placeholder = { Text("Take pills") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )

                    Spacer(Modifier.height(28.dp))
                    Text("Goes off at", fontWeight = FontWeight.SemiBold)
                    Text(
                        "Tap a time to change it. Add more for the same thing morning and night.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(10.dp))
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        times.forEachIndexed { i, t ->
                            InputChip(
                                selected = false,
                                onClick = { picking = i },
                                label = { Text(t, style = MaterialTheme.typography.titleMedium) },
                                // A reminder always keeps one time, so the last chip has no ×.
                                trailingIcon = if (times.size > 1) {
                                    { TextButton(onClick = { times = times.filterIndexed { at, _ -> at != i } }) { Text("×") } }
                                } else null,
                            )
                        }
                        if (times.size < 12) {
                            AssistChip(onClick = { picking = times.size }, label = { Text("+ Add a time") })
                        }
                    }

                    Spacer(Modifier.height(28.dp))
                    Text("Repeats", fontWeight = FontWeight.SemiBold)
                    Text(describeDays(days), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.height(10.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Draft.DAY_LETTERS.forEachIndexed { i, letter ->
                            val on = days and (1 shl i) != 0
                            Surface(
                                shape = CircleShape,
                                color = if (on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceVariant,
                                modifier = Modifier.size(40.dp).clip(CircleShape),
                                onClick = { days = days xor (1 shl i) },
                            ) {
                                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                                    Text(
                                        letter,
                                        color = if (on) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurfaceVariant,
                                        fontWeight = FontWeight.SemiBold,
                                    )
                                }
                            }
                        }
                    }
                    if (days == 0) {
                        Spacer(Modifier.height(6.dp))
                        Text(
                            "With no days picked it goes off once, at the next time above.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }

                    Spacer(Modifier.height(20.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text("Important", fontWeight = FontWeight.SemiBold)
                            Text(
                                "Can speak up during quiet hours on your PC.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        Switch(checked = important, onCheckedChange = { important = it })
                    }

                    if (!draft.isNew) {
                        Spacer(Modifier.height(32.dp))
                        TextButton(onClick = { confirmDelete = true }) {
                            Text("Delete this reminder", color = MaterialTheme.colorScheme.error)
                        }
                    }
                }
            }
        }
    }

    picking?.let { index ->
        val existing = times.getOrNull(index) ?: times.lastOrNull() ?: "09:00"
        val parts = existing.split(":")
        val state = rememberTimePickerState(
            initialHour = parts.getOrNull(0)?.toIntOrNull() ?: 9,
            initialMinute = parts.getOrNull(1)?.toIntOrNull() ?: 0,
            is24Hour = true,
        )
        AlertDialog(
            onDismissRequest = { picking = null },
            confirmButton = {
                TextButton(onClick = {
                    val chosen = "%02d:%02d".format(state.hour, state.minute)
                    times = if (index < times.size) {
                        times.toMutableList().also { it[index] = chosen }
                    } else {
                        times + chosen
                    }.distinct().sorted()
                    picking = null
                }) { Text("Set") }
            },
            dismissButton = { TextButton(onClick = { picking = null }) { Text("Cancel") } },
            text = { TimePicker(state = state) },
        )
    }

    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("Delete this reminder?") },
            text = { Text("It will go from this phone and your PC.") },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    onDelete()
                }) { Text("Delete", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Keep") } },
        )
    }
}
