package dev.scenesync.engine

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ModelTest {
    @Test
    fun decodesFitToWindow() {
        val json = """
            {"protocol_version":1,"device_id":"d","name":"Garage","revision":4,"media":[],
             "mapping":{"corners":[[0.3,0.2],[0.62,0.25],[0.6,0.8],[0.31,0.75]]},"calibrate":true}
        """
        val cfg = SceneSyncJson.decodeFromString(DeviceConfig.serializer(), json)
        assertTrue(cfg.calibrate)
        assertEquals(listOf(0.62, 0.25), cfg.mapping!!.corners[1])
    }

    @Test
    fun olderConfigsHaveNoMapping() {
        val json = """{"protocol_version":1,"device_id":"d","name":"Garage","revision":1,"media":[]}"""
        val cfg = SceneSyncJson.decodeFromString(DeviceConfig.serializer(), json)
        assertNull(cfg.mapping)
        assertFalse(cfg.calibrate)
    }
}
