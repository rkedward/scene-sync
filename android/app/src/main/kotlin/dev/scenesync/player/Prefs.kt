package dev.scenesync.player

import android.content.Context
import java.util.UUID
import kotlin.random.Random

/** Small persistent settings: identity, server address, token, clock offset. */
class Prefs(context: Context) {
    private val sp = context.getSharedPreferences("scenesync", Context.MODE_PRIVATE)

    val deviceId: String
        get() = sp.getString("device_id", null) ?: UUID.randomUUID().toString().also {
            sp.edit().putString("device_id", it).apply()
        }

    var serverUrl: String?
        get() = sp.getString("server_url", null)
        set(v) = sp.edit().putString("server_url", v).apply()

    var token: String?
        get() = sp.getString("token", null)
        set(v) = sp.edit().putString("token", v).apply()

    /** Stays the same across restarts until pairing finishes. */
    val pairingCode: String
        get() = sp.getString("pairing_code", null) ?: "%06d".format(Random.nextInt(1_000_000)).also {
            sp.edit().putString("pairing_code", it).apply()
        }

    fun clearPairingCode() = sp.edit().remove("pairing_code").apply()

    /** Whether we already opened the "Display over other apps" setting once. */
    var askedOverlay: Boolean
        get() = sp.getBoolean("asked_overlay", false)
        set(v) = sp.edit().putBoolean("asked_overlay", v).apply()

    /** Last known server_time - wall_clock, used after a reboot before reaching the server. */
    var wallOffsetMs: Long
        get() = sp.getLong("wall_offset_ms", 0L)
        set(v) = sp.edit().putLong("wall_offset_ms", v).apply()
}
