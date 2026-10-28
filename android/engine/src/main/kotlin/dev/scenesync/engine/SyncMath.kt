package dev.scenesync.engine

import kotlin.math.abs

/**
 * Keeps projectors in a sync group on the same frame. Every projector plays
 * the same clip against the same anchor time on the server clock, so each
 * one can work out where the clip should be right now and nudge itself there.
 */
object SyncMath {
    /** Within this, leave playback alone. */
    const val DEADBAND_MS = 15L
    /** Beyond this, seek instead of changing speed. */
    const val SEEK_THRESHOLD_MS = 400L
    /** Largest speed change used to catch up (5%). */
    const val MAX_SPEED_CHANGE = 0.05

    /**
     * Where the clip should be at [nowMs], or null if it has not started yet.
     * [trimMs] shifts this projector to make up for its own display latency.
     */
    fun expectedPositionMs(anchorMs: Long, nowMs: Long, durationMs: Long?, looping: Boolean, trimMs: Long = 0): Long? {
        val t = nowMs - anchorMs + trimMs
        if (t < 0) return null
        return if (looping && durationMs != null && durationMs > 0) t % durationMs else t
    }

    /** actual - expected; for loops, wrapped to the shorter way round. */
    fun errorMs(actualMs: Long, expectedMs: Long, durationMs: Long?, looping: Boolean): Long {
        var e = actualMs - expectedMs
        if (looping && durationMs != null && durationMs > 0) {
            e %= durationMs
            if (e > durationMs / 2) e -= durationMs
            if (e < -durationMs / 2) e += durationMs
        }
        return e
    }

    sealed class Correction {
        data class Speed(val speed: Float) : Correction()
        data class Seek(val toMs: Long) : Correction()
    }

    /**
     * Small errors are absorbed by playing slightly faster or slower (about a
     * quarter of the error per 250 ms tick, so it settles without
     * overshooting); large ones by seeking a little ahead to cover the seek
     * itself.
     */
    fun correction(errorMs: Long, expectedMs: Long, seekLeadMs: Long, durationMs: Long?, looping: Boolean): Correction {
        val a = abs(errorMs)
        return when {
            a <= DEADBAND_MS -> Correction.Speed(1f)
            a <= SEEK_THRESHOLD_MS -> {
                val change = (errorMs / 1000.0).coerceIn(-MAX_SPEED_CHANGE, MAX_SPEED_CHANGE)
                Correction.Speed((1.0 - change).toFloat())
            }
            else -> {
                var to = expectedMs + seekLeadMs
                if (looping && durationMs != null && durationMs > 0) to %= durationMs
                Correction.Seek(to)
            }
        }
    }
}
