package com.pip.companion

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * The phone's own copy of Pip's data: the same two documents the PC keeps, in the same shape, so
 * the Rust brain reads either without translation.
 *
 * Writes go to a temporary file first and are renamed into place, so a phone that dies mid-write
 * still has yesterday's settings rather than half of today's.
 */
class Store(context: Context) {
    private val dir: File = context.filesDir
    private val settingsFile = File(dir, "settings.json")
    private val stateFile = File(dir, "state.json")
    private val prefs = context.getSharedPreferences("pip", Context.MODE_PRIVATE)

    var settings: JSONObject = read(settingsFile)
        private set
    var state: JSONObject = read(stateFile)
        private set

    private fun read(f: File): JSONObject = try {
        if (f.exists()) JSONObject(f.readText()) else JSONObject()
    } catch (e: Exception) {
        JSONObject() // a damaged file is not worth crashing over; sync will fill it in again
    }

    private fun write(f: File, value: JSONObject) {
        val tmp = File(f.parentFile, f.name + ".tmp")
        tmp.writeText(value.toString())
        if (!tmp.renameTo(f)) {
            f.writeText(value.toString())
            tmp.delete()
        }
    }

    fun saveSettings(value: JSONObject) {
        settings = value
        write(settingsFile, value)
    }

    fun saveState(value: JSONObject) {
        state = value
        write(stateFile, value)
    }

    /** Reminders, newest edit first in the file, shown in time order by the screen. */
    fun reminders(): JSONArray = settings.optJSONArray("reminders") ?: JSONArray()

    fun putReminders(list: JSONArray, now: Long) {
        val next = JSONObject(settings.toString())
        next.put("reminders", list)
        next.put("prefs_updated_at", maxOf(now, settings.optLong("prefs_updated_at")))
        saveSettings(next)
    }

    /**
     * Something the user deleted here. The id is remembered so a device that has been offline
     * can't bring it back.
     */
    fun tombstone(id: String, now: Long) {
        val next = JSONObject(state.toString())
        val gone = next.optJSONObject("sync_deleted") ?: JSONObject()
        gone.put(id, now)
        next.put("sync_deleted", gone)
        saveState(next)
    }

    // --- things only this phone cares about ---------------------------------------------------

    var refreshToken: String?
        get() = prefs.getString("refresh", null)
        set(v) = prefs.edit().apply { if (v == null) remove("refresh") else putString("refresh", v) }.apply()

    /** The shared sync key, once another device has approved this one. */
    var syncKey: String?
        get() = prefs.getString("key", null)
        set(v) = prefs.edit().apply { if (v == null) remove("key") else putString("key", v) }.apply()

    /** The join attempt in flight: id, private seed, when it started, whether it is published. */
    var joining: String?
        get() = prefs.getString("joining", null)
        set(v) = prefs.edit().apply { if (v == null) remove("joining") else putString("joining", v) }.apply()

    var refused: Boolean
        get() = prefs.getBoolean("refused", false)
        set(v) = prefs.edit().putBoolean("refused", v).apply()

    var lastSyncAt: Long
        get() = prefs.getLong("last_sync", 0)
        set(v) = prefs.edit().putLong("last_sync", v).apply()

    fun forgetSync() {
        prefs.edit().remove("refresh").remove("key").remove("joining").remove("refused").remove("last_sync").apply()
    }
}
