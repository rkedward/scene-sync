package dev.scenesync.engine

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import java.time.Instant
import java.time.LocalDate
import java.time.LocalTime

/**
 * Kotlin mirror of the Rust types in `crates/scenesync-core`. Field names
 * match the JSON wire format exactly.
 */

const val PROTOCOL_VERSION = 1

val SceneSyncJson = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    encodeDefaults = true
    classDiscriminator = "type"
}

object InstantSerializer : KSerializer<Instant> {
    override val descriptor = PrimitiveSerialDescriptor("Instant", PrimitiveKind.STRING)
    override fun serialize(encoder: Encoder, value: Instant) = encoder.encodeString(value.toString())
    override fun deserialize(decoder: Decoder): Instant = Instant.parse(decoder.decodeString())
}

object LocalTimeSerializer : KSerializer<LocalTime> {
    override val descriptor = PrimitiveSerialDescriptor("LocalTime", PrimitiveKind.STRING)
    override fun serialize(encoder: Encoder, value: LocalTime) = encoder.encodeString(value.toString())
    // Accepts "HH:MM" and "HH:MM:SS".
    override fun deserialize(decoder: Decoder): LocalTime = LocalTime.parse(decoder.decodeString())
}

object LocalDateSerializer : KSerializer<LocalDate> {
    override val descriptor = PrimitiveSerialDescriptor("LocalDate", PrimitiveKind.STRING)
    override fun serialize(encoder: Encoder, value: LocalDate) = encoder.encodeString(value.toString())
    override fun deserialize(decoder: Decoder): LocalDate = LocalDate.parse(decoder.decodeString())
}

// ---------------------------------------------------------------------------
// Schedule format
// ---------------------------------------------------------------------------

@Serializable
data class Schedule(
    @SerialName("schedule_id") val scheduleId: String,
    val revision: Long,
    val timezone: String,
    val rules: List<Rule> = emptyList(),
    @SerialName("end_date") @Serializable(LocalDateSerializer::class) val endDate: LocalDate? = null,
)

@Serializable
enum class Day {
    @SerialName("mon") MON,
    @SerialName("tue") TUE,
    @SerialName("wed") WED,
    @SerialName("thu") THU,
    @SerialName("fri") FRI,
    @SerialName("sat") SAT,
    @SerialName("sun") SUN;

    val dayOfWeek: java.time.DayOfWeek get() = java.time.DayOfWeek.of(ordinal + 1)
}

@Serializable
data class Rule(
    val id: String,
    val days: List<Day> = emptyList(),
    @Serializable(LocalTimeSerializer::class) val start: LocalTime? = null,
    @Serializable(LocalTimeSerializer::class) val end: LocalTime? = null,
    @Serializable(LocalTimeSerializer::class) val at: LocalTime? = null,
    val action: Action,
    val priority: Int? = null,
    @SerialName("late_policy") val latePolicy: LatePolicy = LatePolicy.SKIP,
    @SerialName("late_grace_ms") val lateGraceMs: Long? = null,
    val resume: ResumePolicy = ResumePolicy.RESTART,
)

@Serializable
sealed class Action {
    abstract val mediaId: String
    val isLoop: Boolean get() = this is Loop

    @Serializable @SerialName("loop")
    data class Loop(@SerialName("media_id") override val mediaId: String) : Action()

    @Serializable @SerialName("play_once")
    data class PlayOnce(@SerialName("media_id") override val mediaId: String) : Action()
}

@Serializable
enum class LatePolicy {
    @SerialName("skip") SKIP,
    @SerialName("join_late") JOIN_LATE,
}

@Serializable
enum class ResumePolicy {
    @SerialName("restart") RESTART,
    @SerialName("continue") CONTINUE,
}

@Serializable
data class Override(
    @SerialName("override_id") val overrideId: String,
    val action: OverrideAction,
    @SerialName("issued_at") @Serializable(InstantSerializer::class) val issuedAt: Instant,
    val hold: Boolean = false,
    @SerialName("max_duration_ms") val maxDurationMs: Long? = null,
)

@Serializable
sealed class OverrideAction {
    @Serializable @SerialName("play")
    data class Play(
        @SerialName("media_id") val mediaId: String,
        val loop: Boolean = false,
    ) : OverrideAction()

    @Serializable @SerialName("stop")
    data object Stop : OverrideAction()
}

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

@Serializable
data class PairRequest(
    @SerialName("device_id") val deviceId: String,
    val code: String,
    val name: String,
    val model: String? = null,
    @SerialName("app_version") val appVersion: String? = null,
    @SerialName("protocol_version") val protocolVersion: Int = PROTOCOL_VERSION,
)

@Serializable
data class PairResponse(val status: String, val token: String? = null)

@Serializable
data class MediaRef(
    @SerialName("media_id") val mediaId: String,
    val sha256: String,
    val size: Long,
    val url: String,
    @SerialName("duration_ms") val durationMs: Long? = null,
)

@Serializable
data class DeviceConfig(
    @SerialName("protocol_version") val protocolVersion: Int,
    @SerialName("device_id") val deviceId: String,
    val name: String,
    val revision: Long,
    val media: List<MediaRef> = emptyList(),
    val schedule: Schedule? = null,
    @SerialName("override") val override: Override? = null,
    val idle: String = "black",
)

@Serializable
data class TimeResponse(@SerialName("server_time_ms") val serverTimeMs: Long)

@Serializable
data class DeviceReport(
    @SerialName("applied_revision") val appliedRevision: Long? = null,
    val state: String = "idle",
    @SerialName("media_id") val mediaId: String? = null,
    @SerialName("position_ms") val positionMs: Long? = null,
    @SerialName("storage_free_bytes") val storageFreeBytes: Long? = null,
    @SerialName("storage_total_bytes") val storageTotalBytes: Long? = null,
    @SerialName("media_bytes") val mediaBytes: Long? = null,
    @SerialName("clock_offset_ms") val clockOffsetMs: Long? = null,
    @SerialName("app_version") val appVersion: String? = null,
    val errors: List<String> = emptyList(),
)

@Serializable
sealed class ServerMessage {
    @Serializable @SerialName("config_changed")
    data class ConfigChanged(val revision: Long) : ServerMessage()
}
