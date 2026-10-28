package dev.scenesync.engine

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class SyncMathTest {
    @Test
    fun expectedPosition() {
        assertNull(SyncMath.expectedPositionMs(10_000, 9_000, 5_000, true))
        assertEquals(2_000, SyncMath.expectedPositionMs(10_000, 12_000, 5_000, true))
        assertEquals(1_000, SyncMath.expectedPositionMs(10_000, 16_000, 5_000, true))
        assertEquals(6_000, SyncMath.expectedPositionMs(10_000, 16_000, 5_000, false))
        assertEquals(1_100, SyncMath.expectedPositionMs(10_000, 16_000, 5_000, true, trimMs = 100))
    }

    @Test
    fun errorWrapsAroundTheLoop() {
        assertEquals(50, SyncMath.errorMs(1_050, 1_000, 5_000, true))
        // Just past the loop point vs. just before it: 100 ms ahead, not 4.9 s behind.
        assertEquals(100, SyncMath.errorMs(50, 4_950, 5_000, true))
        assertEquals(-100, SyncMath.errorMs(4_950, 50, 5_000, true))
        assertEquals(-4_900, SyncMath.errorMs(50, 4_950, 5_000, false))
    }

    @Test
    fun corrections() {
        assertEquals(SyncMath.Correction.Speed(1f), SyncMath.correction(10, 1_000, 150, 5_000, true))
        val slow = SyncMath.correction(40, 1_000, 150, 5_000, true) as SyncMath.Correction.Speed
        assertTrue(slow.speed < 1f && slow.speed >= 0.95f)
        val fast = SyncMath.correction(-300, 1_000, 150, 5_000, true) as SyncMath.Correction.Speed
        assertEquals(1.05f, fast.speed)
        assertEquals(SyncMath.Correction.Seek(1_150), SyncMath.correction(900, 1_000, 150, 5_000, true))
        assertEquals(SyncMath.Correction.Seek(100), SyncMath.correction(900, 4_950, 150, 5_000, true))
    }
}
