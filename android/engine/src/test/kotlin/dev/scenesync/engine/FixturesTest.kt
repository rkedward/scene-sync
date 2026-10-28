package dev.scenesync.engine

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import java.io.File
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** Runs the shared fixtures that the Rust engine also runs. */
class FixturesTest {
    private val file: JsonObject by lazy {
        val path = System.getProperty("scenesync.fixtures") ?: "../../fixtures/schedule/cases.json"
        SceneSyncJson.parseToJsonElement(File(path).readText()).jsonObject
    }

    private fun desiredFrom(o: JsonObject): Desired = when (o["state"]!!.jsonPrimitive.content) {
        "idle" -> Desired.Idle(o["source"]?.jsonPrimitive?.content)
        "play" -> Desired.Play(
            source = o["source"]!!.jsonPrimitive.content,
            anchor = Instant.parse(o["anchor"]!!.jsonPrimitive.content),
            mediaId = o["media_id"]!!.jsonPrimitive.content,
            looping = o["looping"]!!.jsonPrimitive.boolean,
            resume = SceneSyncJson.decodeFromJsonElement(o["resume"]!!),
        )
        else -> error("unknown state")
    }

    @Test
    fun sharedFixtures() {
        val durations = file["durations_ms"]!!.jsonObject.mapValues { it.value.jsonPrimitive.long }
        val schedules = file["schedules"]!!.jsonObject.mapValues { SceneSyncJson.decodeFromJsonElement<Schedule>(it.value) }
        val cases = file["cases"]!!.let { SceneSyncJson.decodeFromJsonElement<List<JsonObject>>(it) }
        assertTrue(cases.isNotEmpty())
        val failures = mutableListOf<String>()
        for (c in cases) {
            val name = c["name"]!!.jsonPrimitive.content
            val schedule = c["schedule"]?.jsonPrimitive?.content?.let { schedules.getValue(it) }
            val override = c["override"]?.let { SceneSyncJson.decodeFromJsonElement<Override>(it) }
            val current = c["current"]?.jsonObject?.let {
                PlaybackKey(it["source"]!!.jsonPrimitive.content, Instant.parse(it["anchor"]!!.jsonPrimitive.content))
            }
            val got = evaluate(schedule, override, durations, Instant.parse(c["now"]!!.jsonPrimitive.content), current)
            val want = desiredFrom(c["expect"]!!.jsonObject)
            if (got != want) failures += "$name:\n  expected $want\n  got      $got"
        }
        assertEquals(emptyList(), failures, failures.joinToString("\n"))
    }

    @Test
    fun parsesServerConfig() {
        val json = """
            {"protocol_version":1,"device_id":"d","name":"Front","revision":3,
             "media":[{"media_id":"ghost","sha256":"ab","size":10,"url":"/api/v1/media/ghost/content","duration_ms":3000}],
             "override":{"override_id":"x","action":{"type":"play","media_id":"ghost","loop":true},
                         "issued_at":"2026-10-28T04:26:33.134603727Z","hold":false},
             "idle":"black"}
        """.trimIndent()
        val cfg = SceneSyncJson.decodeFromString(DeviceConfig.serializer(), json)
        assertEquals(OverrideAction.Play("ghost", loop = true), cfg.override!!.action)
        val msg = SceneSyncJson.decodeFromString(ServerMessage.serializer(), """{"type":"config_changed","revision":4}""")
        assertEquals(ServerMessage.ConfigChanged(4), msg)
    }
}
