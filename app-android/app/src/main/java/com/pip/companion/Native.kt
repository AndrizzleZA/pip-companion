package com.pip.companion

import org.json.JSONObject

/**
 * Pip's brain, compiled for the phone.
 *
 * The merge rules and the encryption are the same Rust code the PC runs — there is no second
 * implementation to drift. One call crosses the boundary: a name and a JSON object in, JSON out.
 */
object Native {
    init {
        System.loadLibrary("pipjni")
    }

    private external fun call(name: String, args: String): String

    /** Runs one operation. Throws [PipError] with a sentence the user can read. */
    fun ask(name: String, args: JSONObject): JSONObject {
        val reply = call(name, args.toString())
        val out = JSONObject(reply)
        if (out.has("error")) throw PipError(out.getString("error"))
        return out
    }

    /** True once the library is loaded and answering. */
    fun works(): Boolean = try {
        ask("join_stale", JSONObject().put("request", JSONObject().put("created_at", 0)).put("now", 0))
        true
    } catch (e: Throwable) {
        false
    }
}

class PipError(message: String) : Exception(message)
