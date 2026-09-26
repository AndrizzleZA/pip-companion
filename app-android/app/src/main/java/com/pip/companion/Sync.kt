package com.pip.companion

import org.json.JSONObject

/**
 * One sync round, the phone's half of what `drivesync.rs` does on the PC — deliberately the same
 * shape, because both sides have to agree about the same folder.
 *
 * Adding this phone takes one tap on the other device: it leaves a join request, the PC shows
 * "… wants to sync — Approve", and the answer is sealed so only this phone can open it. Nothing
 * is typed anywhere.
 */
class Sync(private val store: Store, private val deviceName: String) {

    data class Outcome(
        val synced: Boolean = false,
        val lostRace: Boolean = false,
        val waiting: Boolean = false,
        val refused: Boolean = false,
        val joined: Boolean = false,
        val signedOut: Boolean = false,
        val asking: List<JSONObject> = emptyList(),
        val error: String? = null,
    )

    companion object {
        const val DOC = "pip-sync.bin"
        const val JOIN = "pip-join-"
        const val GRANT = "pip-grant-"
        const val DENY = "pip-no-"
        fun joinName(id: String) = "$JOIN$id.bin"
        fun grantName(id: String) = "$GRANT$id.bin"
        fun denyName(id: String) = "$DENY$id.bin"
    }

    private var token: Token? = null

    /** Runs a round. Never throws: everything comes back as a sentence in [Outcome.error]. */
    fun round(approve: JSONObject? = null, deny: List<String> = emptyList()): Outcome = try {
        go(approve, deny)
    } catch (e: SignedOut) {
        Outcome(signedOut = true, error = "Google signed Pip out. Connect again to keep syncing.")
    } catch (e: PipError) {
        Outcome(error = e.message)
    } catch (e: Exception) {
        Outcome(error = "Couldn't reach Google just now. Pip will try again.")
    }

    private fun go(approve: JSONObject?, deny: List<String>): Outcome {
        val refresh = store.refreshToken ?: return Outcome(error = "Sync isn't connected on this phone yet")
        val access = accessToken(refresh)
        val now = Auth.now()
        var files = Drive.list(access)
        fun idOf(name: String) = files.entries.firstOrNull { it.value == name }?.key
        // Drive keeps two files of the same name as two files, so two devices that start a sync in
        // the same moment each get one. Every device agrees on the lowest id; the rest are rivals.
        fun docs() = files.entries.filter { it.value == DOC }.map { it.key }.sorted()

        // Whatever the user tapped goes first.
        if (approve != null) {
            val key = store.syncKey
            if (key != null) {
                val sealed = Native.ask(
                    "join_grant",
                    JSONObject().put("request", approve).put("key", key)
                        .put("random", Auth.randomHex(32)).put("nonce", Auth.randomHex(24)),
                ).getString("bytes")
                Drive.upload(access, grantName(approve.getString("id")), sealed.fromHex(), idOf(grantName(approve.getString("id"))))
                idOf(joinName(approve.getString("id")))?.let { Drive.delete(access, it) }
            }
        }
        for (id in deny) {
            val marker = JSONObject().put("id", id).put("device", "").put("public_key", "").put("created_at", now)
            Drive.upload(access, denyName(id), marker.toString().toByteArray(), idOf(denyName(id)))
            idOf(joinName(id))?.let { Drive.delete(access, it) }
        }
        if (approve != null || deny.isNotEmpty()) files = Drive.list(access)

        // No key yet: has anyone answered, or should this phone ask?
        var key = store.syncKey
        if (key == null) {
            if (store.refused) return Outcome(refused = true)
            val attempt = attempt(now)
            val answer = idOf(grantName(attempt.id))
            val refusal = idOf(denyName(attempt.id))
            val asked = idOf(joinName(attempt.id))
            when {
                answer != null -> {
                    val bytes = Drive.fetch(access, answer)
                    key = Native.ask("join_accept", JSONObject().put("seed", attempt.seed).put("bytes", bytes.toHex())).getString("key")
                    store.syncKey = key
                    store.joining = null
                    Drive.delete(access, answer)
                    asked?.let { Drive.delete(access, it) }
                }
                refusal != null -> {
                    store.refused = true
                    store.joining = null
                    Drive.delete(access, refusal)
                    asked?.let { Drive.delete(access, it) }
                    return Outcome(refused = true)
                }
                // Only keep waiting while there is actually a sync to be let into. If it has gone,
                // waiting would mean waiting for an approval nobody can give.
                docs().isEmpty() -> {
                    asked?.let { Drive.delete(access, it) }
                    key = Auth.randomHex(32)
                    store.syncKey = key
                    store.joining = null
                }
                asked != null -> return Outcome(waiting = true)
                else -> {
                    publish(access, attempt, null)
                    return Outcome(waiting = true)
                }
            }
        }

        // This phone holds the key, so it can let other devices in too.
        val asking = mutableListOf<JSONObject>()
        for ((id, name) in files) {
            if (!name.startsWith(JOIN)) continue
            val req = try {
                JSONObject(String(Drive.fetch(access, id)))
            } catch (e: Exception) {
                continue
            }
            val stale = Native.ask("join_stale", JSONObject().put("request", req).put("now", now)).getBoolean("stale")
            if (stale) Drive.delete(access, id) else asking.add(req)
        }

        // The document itself: merge, and write back only if this phone has something to add.
        val local = Native.ask(
            "local_doc",
            JSONObject().put("settings", store.settings).put("state", store.state).put("device", deviceName).put("now", now),
        )
        files = Drive.list(access)
        val all = docs()
        val docId = all.firstOrNull()
        val remote = docId?.let {
            try {
                Native.ask("open", JSONObject().put("key", key).put("bytes", Drive.fetch(access, it).toHex()))
            } catch (e: PipError) {
                // The agreed document isn't ours: another device won the start. Stand down and ask
                // to be let in — what is here merges upward once we are.
                store.syncKey = null
                store.joining = null
                return Outcome(lostRace = true, waiting = true)
            }
        }
        // We can read it, so any other copy is a rival that lost.
        for (rival in all.drop(1)) Drive.delete(access, rival)
        val merged = if (remote == null) local else Native.ask("merge", JSONObject().put("local", local).put("remote", remote))
        if (remote == null || remote.toString() != merged.toString()) {
            val sealed = Native.ask("seal", JSONObject().put("doc", merged).put("key", key).put("nonce", Auth.randomHex(24))).getString("bytes")
            Drive.upload(access, DOC, sealed.fromHex(), docId)
        }
        val applied = Native.ask("apply", JSONObject().put("doc", merged).put("settings", store.settings).put("state", store.state))
        store.saveSettings(applied.getJSONObject("settings"))
        store.saveState(applied.getJSONObject("state"))
        store.lastSyncAt = now
        return Outcome(synced = true, joined = store.syncKey != null, asking = asking)
    }

    private fun publish(access: String, attempt: Attempt, existingId: String?) {
        val req = Native.ask(
            "join_request",
            JSONObject().put("seed", attempt.seed).put("id", attempt.id).put("device", deviceName).put("now", attempt.createdAt),
        )
        Drive.upload(access, joinName(attempt.id), req.toString().toByteArray(), existingId)
        store.joining = "${attempt.id}|${attempt.seed}|${attempt.createdAt}|1"
    }

    private data class Attempt(val id: String, val seed: String, val createdAt: Long, val published: Boolean)

    /** The attempt in flight, or a fresh one if there is none or the old one has expired. */
    private fun attempt(now: Long): Attempt {
        val saved = store.joining?.split("|")
        if (saved != null && saved.size == 4) {
            val createdAt = saved[2].toLongOrNull() ?: 0
            if (now - createdAt <= 15 * 60) return Attempt(saved[0], saved[1], createdAt, saved[3] == "1")
        }
        val fresh = Attempt(Auth.randomHex(5), Auth.randomHex(32), now, false)
        store.joining = "${fresh.id}|${fresh.seed}|${fresh.createdAt}|0"
        return fresh
    }

    /** "Ask again" after a refusal: a brand-new attempt, with a new key pair. */
    fun askAgain() {
        store.refused = false
        store.joining = null
    }

    private fun accessToken(refresh: String): String {
        val current = token
        if (current != null && current.expiresAt > Auth.now() + 60) return current.access
        val fresh = Auth.refresh(refresh)
        token = fresh
        return fresh.access
    }
}
