package com.pip.companion

import android.net.Uri
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64

/** Google signed Pip out; the user has to connect again. */
class SignedOut(message: String) : Exception(message)

/**
 * Signing in, the way Google asks phone apps to do it: the consent screen opens in the phone's own
 * browser, Pip never sees the password, and there is no client secret on the device — the app's
 * signature is the proof. PKCE ties the answer to this one attempt.
 */
object Auth {
    private const val AUTH = "https://accounts.google.com/o/oauth2/v2/auth"
    private const val TOKEN = "https://oauth2.googleapis.com/token"
    private const val REVOKE = "https://oauth2.googleapis.com/revoke"
    const val REDIRECT = "com.pip.companion:/oauth2redirect"
    /** The hidden per-app folder, and nothing else in the account. */
    const val SCOPE = "https://www.googleapis.com/auth/drive.appdata"

    fun randomUrlSafe(bytes: Int): String {
        val b = ByteArray(bytes)
        SecureRandom().nextBytes(b)
        return Base64.getUrlEncoder().withoutPadding().encodeToString(b)
    }

    fun randomHex(bytes: Int): String {
        val b = ByteArray(bytes)
        SecureRandom().nextBytes(b)
        return b.joinToString("") { "%02x".format(it) }
    }

    private fun challenge(verifier: String): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray())
        return Base64.getUrlEncoder().withoutPadding().encodeToString(digest)
    }

    fun consentUrl(verifier: String, state: String): String =
        Uri.parse(AUTH).buildUpon()
            .appendQueryParameter("client_id", BuildConfig.GOOGLE_CLIENT_ID)
            .appendQueryParameter("redirect_uri", REDIRECT)
            .appendQueryParameter("response_type", "code")
            .appendQueryParameter("scope", SCOPE)
            .appendQueryParameter("code_challenge", challenge(verifier))
            .appendQueryParameter("code_challenge_method", "S256")
            .appendQueryParameter("state", state)
            .appendQueryParameter("prompt", "consent")
            .build().toString()

    /** Returns the refresh token and a fresh access token. */
    fun exchange(code: String, verifier: String): Pair<String, Token> {
        val form = "code=${Uri.encode(code)}&client_id=${Uri.encode(BuildConfig.GOOGLE_CLIENT_ID)}" +
            "&redirect_uri=${Uri.encode(REDIRECT)}&grant_type=authorization_code&code_verifier=${Uri.encode(verifier)}"
        val json = post(TOKEN, form)
        val refresh = json.optString("refresh_token")
        if (refresh.isEmpty()) throw PipError("Google didn't finish the sign-in. Please try again.")
        return refresh to Token(json.getString("access_token"), now() + json.optLong("expires_in", 3600))
    }

    fun refresh(refreshToken: String): Token {
        val form = "refresh_token=${Uri.encode(refreshToken)}&client_id=${Uri.encode(BuildConfig.GOOGLE_CLIENT_ID)}&grant_type=refresh_token"
        val json = try {
            post(TOKEN, form)
        } catch (e: PipError) {
            if (e.message?.contains("invalid_grant") == true) throw SignedOut("Google signed Pip out")
            throw e
        }
        return Token(json.getString("access_token"), now() + json.optLong("expires_in", 3600))
    }

    fun revoke(refreshToken: String) {
        try {
            post(REVOKE, "token=${Uri.encode(refreshToken)}")
        } catch (e: Exception) {
            // Disconnecting locally matters more than the round trip; Google expires it anyway.
        }
    }

    private fun post(url: String, form: String): JSONObject {
        val c = URL(url).openConnection() as HttpURLConnection
        c.requestMethod = "POST"
        c.doOutput = true
        c.connectTimeout = 15000
        c.readTimeout = 20000
        c.setRequestProperty("Content-Type", "application/x-www-form-urlencoded")
        c.outputStream.use { it.write(form.toByteArray()) }
        val ok = c.responseCode in 200..299
        val body = (if (ok) c.inputStream else c.errorStream)?.use { String(it.readBytes()) } ?: ""
        c.disconnect()
        if (!ok) throw PipError(if (body.contains("invalid_grant")) "invalid_grant" else "Google answered ${c.responseCode}")
        return JSONObject(body)
    }

    fun now(): Long = System.currentTimeMillis() / 1000
}

data class Token(val access: String, val expiresAt: Long)

/**
 * Pip's own folder in the user's Drive: invisible in Drive itself, unreadable by other apps, and
 * the only thing this permission opens. Same files, same names as on the PC.
 */
object Drive {
    private const val API = "https://www.googleapis.com/drive/v3"
    private const val UPLOAD = "https://www.googleapis.com/upload/drive/v3"
    private const val BOUNDARY = "pipsyncboundary"
    private const val MAX_BYTES = 2 * 1024 * 1024

    /** Everything in the folder, as id to name. A round lists once and works from that. */
    fun list(token: String): Map<String, String> {
        val body = get("$API/files?spaces=appDataFolder&fields=files(id,name)&pageSize=100", token)
        val files: JSONArray = JSONObject(String(body)).optJSONArray("files") ?: JSONArray()
        val out = LinkedHashMap<String, String>()
        for (i in 0 until files.length()) {
            val f = files.getJSONObject(i)
            out[f.optString("id")] = f.optString("name")
        }
        return out
    }

    fun fetch(token: String, id: String): ByteArray = get("$API/files/$id?alt=media", token)

    fun delete(token: String, id: String) {
        val c = open("$API/files/$id", token, "DELETE")
        val code = c.responseCode
        c.disconnect()
        if (code == 401 || code == 403) throw SignedOut("Google signed Pip out of Drive")
    }

    fun upload(token: String, name: String, bytes: ByteArray, existingId: String?) {
        val meta = if (existingId != null) """{"name":"$name"}""" else """{"name":"$name","parents":["appDataFolder"]}"""
        val out = ByteArrayOutputStream()
        out.write("--$BOUNDARY\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n$meta\r\n".toByteArray())
        out.write("--$BOUNDARY\r\nContent-Type: application/octet-stream\r\n\r\n".toByteArray())
        out.write(bytes)
        out.write("\r\n--$BOUNDARY--\r\n".toByteArray())

        val url = if (existingId != null) "$UPLOAD/files/$existingId?uploadType=multipart" else "$UPLOAD/files?uploadType=multipart"
        val c = open(url, token, if (existingId != null) "PATCH" else "POST")
        c.doOutput = true
        c.setRequestProperty("Content-Type", "multipart/related; boundary=$BOUNDARY")
        c.outputStream.use { it.write(out.toByteArray()) }
        val code = c.responseCode
        c.disconnect()
        when {
            code in 200..299 -> {}
            code == 401 || code == 403 -> throw SignedOut("Google signed Pip out of Drive")
            else -> throw PipError("Drive wouldn't save the sync file (HTTP $code)")
        }
    }

    private fun get(url: String, token: String): ByteArray {
        val c = open(url, token, "GET")
        val code = c.responseCode
        if (code == 401 || code == 403) {
            c.disconnect()
            throw SignedOut("Google signed Pip out of Drive")
        }
        if (code !in 200..299) {
            c.disconnect()
            throw PipError("Drive answered HTTP $code")
        }
        val body = c.inputStream.use { it.readBytes() }
        c.disconnect()
        if (body.size > MAX_BYTES) throw PipError("The sync file is unexpectedly large; ignoring it")
        return body
    }

    private fun open(url: String, token: String, method: String): HttpURLConnection {
        val c = URL(url).openConnection() as HttpURLConnection
        c.requestMethod = method
        c.connectTimeout = 15000
        c.readTimeout = 20000
        c.setRequestProperty("Authorization", "Bearer $token")
        return c
    }
}

fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }

fun String.fromHex(): ByteArray = chunked(2).map { it.toInt(16).toByte() }.toByteArray()
