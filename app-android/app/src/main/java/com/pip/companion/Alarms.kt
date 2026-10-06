package com.pip.companion

import android.app.AlarmManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import org.json.JSONObject
import java.util.TimeZone

/**
 * Reminders that behave like the phone's own alarm clock.
 *
 * Android only treats `setAlarmClock` as a real alarm: it fires at the minute, gets out of Doze to
 * do it, and puts the alarm icon in the status bar so the phone tells the truth about what it will
 * do. Repeating alarms are deliberately inexact, so Pip arms exactly one — the next thing due — and
 * arms the one after it when that fires. The engine that decides "what is next" is the same Rust
 * the PC runs, so both devices ring at the same minute.
 */
object Alarms {
    const val CHANNEL = "reminders"
    /// Birthdays and anniversaries. Deliberately a separate channel: a birthday is worth knowing
    /// about, not worth an alarm clock, and keeping them apart lets either be silenced alone.
    const val CHANNEL_DATES = "dates"
    private const val RING = 1001          // the one pending alarm
    const val ACTION_RING = "com.pip.companion.RING"
    const val ACTION_SNOOZE = "com.pip.companion.SNOOZE"
    const val ACTION_DONE = "com.pip.companion.DONE"
    const val ACTION_DATE_DONE = "com.pip.companion.DATE_DONE"
    const val EXTRA_ID = "id"
    const val EXTRA_TITLE = "title"
    const val EXTRA_OCCURRENCE = "occurrence"

    fun channel(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL) != null) return
        val c = NotificationChannel(CHANNEL, "Reminders", NotificationManager.IMPORTANCE_HIGH)
        c.description = "Your reminders, at the time you set them"
        c.enableVibration(true)
        c.setBypassDnd(false)
        // Alarm usage: it plays at alarm volume and survives "ring/vibrate" being turned down.
        c.setSound(
            Settings.System.DEFAULT_ALARM_ALERT_URI ?: Settings.System.DEFAULT_NOTIFICATION_URI,
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ALARM)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build(),
        )
        nm.createNotificationChannel(c)
    }

    /// The quieter channel, for dates.
    fun datesChannel(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL_DATES) != null) return
        val c = NotificationChannel(CHANNEL_DATES, "Birthdays and dates", NotificationManager.IMPORTANCE_DEFAULT)
        c.description = "A note on the morning of someone's birthday"
        nm.createNotificationChannel(c)
    }

    /**
     * A date, as a note rather than an alarm.
     *
     * No full-screen takeover and no alarm sound: a birthday arriving at 09:00 should not behave
     * like the thing that gets you out of bed. Tapping it opens Pip; "Got it" settles it here and,
     * through the ordinary sync, on the PC as well.
     */
    fun note(context: Context, id: String, title: String, body: String, occurrence: Long) {
        datesChannel(context)
        val nm = context.getSystemService(NotificationManager::class.java)
        val open = PendingIntent.getActivity(
            context,
            occurrence.toInt(),
            Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(context, CHANNEL_DATES)
            .setSmallIcon(R.drawable.ic_pip_notification)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setCategory(NotificationCompat.CATEGORY_REMINDER)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
            .setAutoCancel(true)
            .setContentIntent(open)
            .addAction(0, "Got it", action(context, ACTION_DATE_DONE, id, title, occurrence))
        if (NotificationManagerCompat.from(context).areNotificationsEnabled()) {
            nm.notify(noteId(occurrence), notification.build())
        }
    }

    /// "Got it" on a date. Same store, same sync, so the PC stops asking about it too.
    fun dateDone(context: Context, store: Store, id: String, occurrence: Long) {
        try {
            val state = Native.ask(
                "ack_date",
                JSONObject().put("state", store.state).put("id", id).put("occurrence", occurrence),
            )
            store.saveState(state)
        } catch (e: Exception) {
            return
        }
        clear(context, occurrence)
        arm(context, store)
    }

    private fun args(store: Store, now: Long): JSONObject = JSONObject()
        .put("settings", store.settings)
        .put("state", store.state)
        .put("now", now)
        .put("tz_offset_min", TimeZone.getDefault().getOffset(now * 1000) / 60000)

    /** Arms one alarm for the next thing due. Call after any change: edits, sync, a reminder going off. */
    fun arm(context: Context, store: Store) {
        channel(context)
        val now = Auth.now()
        val am = context.getSystemService(AlarmManager::class.java)
        val at = try {
            val answer = Native.ask("next_alert", args(store, now))
            if (answer.isNull("at")) {
                cancel(context, am) // no reminders: no alarm icon, no wake-ups
                return
            }
            answer.getLong("at")
        } catch (e: Exception) {
            return
        }
        val fire = PendingIntent.getBroadcast(
            context,
            RING,
            Intent(context, AlarmReceiver::class.java).setAction(ACTION_RING),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        // Tapping the alarm icon in the status bar opens Pip.
        val show = PendingIntent.getActivity(
            context,
            RING + 1,
            Intent(context, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val whenMs = at * 1000
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S || am.canScheduleExactAlarms()) {
            am.setAlarmClock(AlarmManager.AlarmClockInfo(whenMs, show), fire)
        } else {
            // Exact alarms turned off for Pip: still fire, just without the promise of the minute.
            am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, whenMs, fire)
        }
    }

    /** Everything due at this moment, straight from the engine. */
    fun due(store: Store, now: Long = Auth.now()): List<JSONObject> = try {
        val items = Native.ask("due", args(store, now)).getJSONArray("items")
        (0 until items.length()).map { items.getJSONObject(it) }
    } catch (e: Exception) {
        emptyList()
    }

    /**
     * Rings one reminder. Locked phone: the alarm screen, like a morning alarm. Otherwise a
     * heads-up notification with the same two buttons, so nothing takes over the screen while the
     * phone is in use.
     */
    fun ring(context: Context, id: String, title: String, occurrence: Long, late: Boolean = false) {
        channel(context)
        val nm = context.getSystemService(NotificationManager::class.java)
        val full = Intent(context, AlarmActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
            .putExtra(EXTRA_ID, id)
            .putExtra(EXTRA_TITLE, title)
            .putExtra(EXTRA_OCCURRENCE, occurrence)
        val fullPending = PendingIntent.getActivity(
            context,
            occurrence.toInt(),
            full,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        // Something hours old shouldn't go off like a fresh alarm — it's a note that it was missed.
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_pip_notification)
            .setContentTitle(title)
            .setContentText(if (late) "Earlier today" else "Reminder")
            .setCategory(if (late) NotificationCompat.CATEGORY_REMINDER else NotificationCompat.CATEGORY_ALARM)
            .setPriority(if (late) NotificationCompat.PRIORITY_DEFAULT else NotificationCompat.PRIORITY_MAX)
            .setSilent(late)
            .setAutoCancel(true)
            .setOngoing(false)
            .setContentIntent(fullPending)
            .addAction(0, "Snooze", action(context, ACTION_SNOOZE, id, title, occurrence))
            .addAction(0, "Done", action(context, ACTION_DONE, id, title, occurrence))

        // Android 14 lets people take full-screen alerts away; honour that and just buzz instead.
        val canTakeOver = Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE || nm.canUseFullScreenIntent()
        if (canTakeOver && !late) notification.setFullScreenIntent(fullPending, true)

        if (NotificationManagerCompat.from(context).areNotificationsEnabled()) {
            nm.notify(noteId(occurrence), notification.build())
        }
    }

    private fun cancel(context: Context, am: AlarmManager) {
        val fire = PendingIntent.getBroadcast(
            context,
            RING,
            Intent(context, AlarmReceiver::class.java).setAction(ACTION_RING),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        am.cancel(fire)
    }

    fun clear(context: Context, occurrence: Long) {
        context.getSystemService(NotificationManager::class.java).cancel(noteId(occurrence))
    }

    private fun noteId(occurrence: Long): Int = (occurrence % Int.MAX_VALUE).toInt()

    private fun action(context: Context, what: String, id: String, title: String, occurrence: Long): PendingIntent =
        PendingIntent.getBroadcast(
            context,
            (occurrence.toInt() * 31) + what.hashCode(),
            Intent(context, AlarmReceiver::class.java)
                .setAction(what)
                .putExtra(EXTRA_ID, id)
                .putExtra(EXTRA_TITLE, title)
                .putExtra(EXTRA_OCCURRENCE, occurrence),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    /** "Done" — this occurrence is answered. Writes through so the PC hears about it too. */
    fun done(context: Context, store: Store, id: String, occurrence: Long) {
        val now = Auth.now()
        try {
            val out = Native.ask(
                "ack",
                JSONObject().put("settings", store.settings).put("state", store.state)
                    .put("id", id).put("occurrence", occurrence).put("now", now),
            )
            store.saveSettings(out.getJSONObject("settings"))
            store.saveState(out.getJSONObject("state"))
        } catch (e: Exception) {
            return
        }
        clear(context, occurrence)
        arm(context, store)
    }

    /** "Snooze" — ask again shortly. The length is the one set in Pip's settings. */
    fun snooze(context: Context, store: Store, id: String, occurrence: Long) {
        val minutes = store.settings.optJSONObject("notify")?.optInt("snooze_min", 10) ?: 10
        val until = Auth.now() + minutes * 60
        try {
            val state = Native.ask("snooze", JSONObject().put("state", store.state).put("id", id).put("until", until))
            store.saveState(state)
        } catch (e: Exception) {
            return
        }
        clear(context, occurrence)
        arm(context, store)
    }
}

/** The alarm going off, and the two things you can say back to it. */
class AlarmReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val store = Store(context)
        val id = intent.getStringExtra(Alarms.EXTRA_ID).orEmpty()
        val title = intent.getStringExtra(Alarms.EXTRA_TITLE).orEmpty()
        val occurrence = intent.getLongExtra(Alarms.EXTRA_OCCURRENCE, 0)
        when (intent.action) {
            Alarms.ACTION_SNOOZE -> Alarms.snooze(context, store, id, occurrence)
            Alarms.ACTION_DONE -> Alarms.done(context, store, id, occurrence)
            Alarms.ACTION_DATE_DONE -> Alarms.dateDone(context, store, id, occurrence)
            else -> {
                // Ring whatever is due, then arm the next one — the alarm we just used is spent.
                // Each kind is shown the way it deserves, and anything this screen doesn't know
                // how to show is skipped rather than rung as a blank alarm.
                for (item in Alarms.due(store)) {
                    val id = item.optString("id")
                    val title = item.optString("title")
                    if (title.isEmpty()) continue
                    when (item.optString("kind")) {
                        "reminder" -> Alarms.ring(context, id, title, item.optLong("occurrence"), item.optBoolean("late"))
                        "date" -> Alarms.note(context, id, title, item.optString("body"), item.optLong("occurrence"))
                    }
                }
                Alarms.arm(context, store)
            }
        }
    }
}

/** Android forgets alarms when the phone restarts, so Pip puts them back. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED || intent.action == Intent.ACTION_MY_PACKAGE_REPLACED) {
            Alarms.arm(context, Store(context))
        }
    }
}

/** Someone turned exact alarms back on (or off) for Pip; re-arm with whatever we may now use. */
class ExactAlarmReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        Alarms.arm(context, Store(context))
    }
}

/** Opens the phone's "Alarms & reminders" screen for Pip. */
fun exactAlarmSettings(context: Context): Intent =
    Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM, Uri.parse("package:" + context.packageName))
