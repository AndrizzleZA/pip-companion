package com.pip.companion

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.Ringtone
import android.media.RingtoneManager
import android.os.Build
import android.os.Bundle
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * The alarm itself: title, time, and two things to do about it.
 *
 * It shows over the lock screen, turns the screen on and keeps it on, the way a morning alarm does,
 * and it rings at alarm volume until it is answered. It gives up after a minute — long enough to be
 * heard from another room, short enough not to be ringing in an empty house — and leaves its
 * notification behind so the reminder isn't simply lost.
 */
class AlarmActivity : ComponentActivity() {
    private var ringtone: Ringtone? = null
    private var vibrator: Vibrator? = null
    private val giveUp = android.os.Handler(android.os.Looper.getMainLooper())
    private var giveUpAt: Runnable? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        showOverLockScreen()

        val id = intent.getStringExtra(Alarms.EXTRA_ID).orEmpty()
        val title = intent.getStringExtra(Alarms.EXTRA_TITLE).orEmpty().ifEmpty { "Reminder" }
        val occurrence = intent.getLongExtra(Alarms.EXTRA_OCCURRENCE, Auth.now())
        val store = Store(this)
        val snoozeMinutes = store.settings.optJSONObject("notify")?.optInt("snooze_min", 10) ?: 10

        startRinging()
        // Ring for a minute, then stop: the notification stays, so it can still be answered.
        giveUpAt = Runnable {
            stopRinging()
            finish()
        }.also { giveUp.postDelayed(it, RING_FOR_MS) }
        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) {
                Alarm(
                    title = title,
                    at = SimpleDateFormat("HH:mm", Locale.getDefault()).format(Date(occurrence * 1000)),
                    snoozeMinutes = snoozeMinutes,
                    onSnooze = {
                        stopRinging()
                        Alarms.snooze(this, store, id, occurrence)
                        finish()
                    },
                    onDone = {
                        stopRinging()
                        Alarms.done(this, store, id, occurrence)
                        finish()
                    },
                )
            }
        }
    }

    private fun showOverLockScreen() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        } else {
            @Suppress("DEPRECATION")
            window.addFlags(
                android.view.WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or
                    android.view.WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON,
            )
        }
        // Without this the phone falls asleep again a few seconds later and the alarm is missed.
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        // Ask the keyguard to step aside so Snooze and Done are one tap, not unlock-then-tap.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            getSystemService(android.app.KeyguardManager::class.java)?.requestDismissKeyguard(this, null)
        }
    }

    private fun startRinging() {
        val uri = Settings.System.DEFAULT_ALARM_ALERT_URI
            ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM)
            ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_NOTIFICATION)
        ringtone = RingtoneManager.getRingtone(this, uri)?.apply {
            audioAttributes = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ALARM)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build()
            isLooping = true
            play()
        }
        val v = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        }
        vibrator = v
        val pattern = longArrayOf(0, 500, 800)
        v?.vibrate(VibrationEffect.createWaveform(pattern, 0), AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ALARM).build())
    }

    private fun stopRinging() {
        ringtone?.takeIf { it.isPlaying }?.stop()
        ringtone = null
        vibrator?.cancel()
        vibrator = null
    }

    override fun onDestroy() {
        super.onDestroy()
        giveUpAt?.let { giveUp.removeCallbacks(it) }
        stopRinging()
    }

    private companion object {
        const val RING_FOR_MS = 60_000L
    }
}

@Composable
private fun Alarm(title: String, at: String, snoozeMinutes: Int, onSnooze: () -> Unit, onDone: () -> Unit) {
    Surface(Modifier.fillMaxSize()) {
        Column(
            Modifier.fillMaxSize().padding(28.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            Text(at, style = MaterialTheme.typography.displayMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(16.dp))
            Text(
                title,
                style = MaterialTheme.typography.headlineLarge,
                fontWeight = FontWeight.SemiBold,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(56.dp))
            Button(onClick = onDone, modifier = Modifier.fillMaxWidth().height(56.dp)) {
                Text("Done", style = MaterialTheme.typography.titleMedium)
            }
            Spacer(Modifier.height(12.dp))
            OutlinedButton(onClick = onSnooze, modifier = Modifier.fillMaxWidth().height(56.dp)) {
                Text("Snooze $snoozeMinutes min", style = MaterialTheme.typography.titleMedium)
            }
        }
    }
}
