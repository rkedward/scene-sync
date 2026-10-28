package dev.scenesync.player

import android.os.SystemClock
import java.time.Instant

/**
 * Server time, estimated without touching the system clock (apps cannot set it).
 *
 * After a sync, time = elapsedRealtime + offset. elapsedRealtime is monotonic,
 * so wall-clock changes on the projector do not affect playback. After a
 * reboot with no server, it falls back to the wall clock plus the last known
 * offset.
 */
class ServerClock(private val prefs: Prefs) {
    @Volatile private var elapsedOffsetMs: Long? = null

    val synced: Boolean get() = elapsedOffsetMs != null

    fun now(): Instant {
        val off = elapsedOffsetMs
        return if (off != null) {
            Instant.ofEpochMilli(SystemClock.elapsedRealtime() + off)
        } else {
            Instant.ofEpochMilli(System.currentTimeMillis() + prefs.wallOffsetMs)
        }
    }

    /** Offset of server time from this device's wall clock, for reports. */
    fun wallOffsetMs(): Long = now().toEpochMilli() - System.currentTimeMillis()

    /**
     * Records one round trip. [sentElapsed]/[recvElapsed] are elapsedRealtime
     * before and after the request.
     */
    fun update(serverMs: Long, sentElapsed: Long, recvElapsed: Long) {
        val midElapsed = (sentElapsed + recvElapsed) / 2
        elapsedOffsetMs = serverMs - midElapsed
        val midWall = System.currentTimeMillis() - (recvElapsed - midElapsed)
        prefs.wallOffsetMs = serverMs - midWall
    }
}
