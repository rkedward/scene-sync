package dev.scenesync.engine

import java.time.Duration
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.ZoneId
import java.time.ZonedDateTime
import java.time.temporal.ChronoUnit

/**
 * Kotlin port of `crates/scenesync-core/src/schedule.rs`. Both must pass
 * `fixtures/schedule/cases.json`; see that file and the Rust module docs for
 * the rules (override beats rules, priority then latest start then rule
 * order, one-shots skipped when more than 5 s late, DST handling).
 */

const val DEFAULT_LATE_GRACE_MS = 5_000L
const val DEFAULT_LOOP_PRIORITY = 0
const val DEFAULT_ONE_SHOT_PRIORITY = 10

data class PlaybackKey(val source: String, val anchor: Instant)

sealed class Desired {
    abstract val key: PlaybackKey?

    data class Idle(val source: String? = null) : Desired() {
        override val key: PlaybackKey? get() = null
    }

    data class Play(
        val source: String,
        val anchor: Instant,
        val mediaId: String,
        val looping: Boolean,
        val resume: ResumePolicy,
    ) : Desired() {
        override val key: PlaybackKey get() = PlaybackKey(source, anchor)
    }
}

private data class Occurrence(val ruleIndex: Int, val start: Instant, val end: Instant)

/** Local wall-clock time to an instant: gaps move to the next valid minute, overlaps use the first occurrence. */
fun resolveLocal(zone: ZoneId, date: LocalDate, time: LocalTime): Instant {
    var ldt = LocalDateTime.of(date, time)
    repeat(24 * 60) {
        if (zone.rules.getValidOffsets(ldt).isNotEmpty()) {
            // With no preferred offset, ofLocal picks the earlier instant in an overlap.
            return ZonedDateTime.ofLocal(ldt, zone, null).toInstant()
        }
        ldt = ldt.truncatedTo(ChronoUnit.MINUTES).plusMinutes(1)
    }
    return LocalDateTime.of(date, time).toInstant(java.time.ZoneOffset.UTC)
}

private fun Schedule.zoneOrNull(): ZoneId? = try {
    ZoneId.of(timezone)
} catch (e: Exception) {
    null
}

private fun occurrence(
    schedule: Schedule,
    zone: ZoneId,
    index: Int,
    rule: Rule,
    date: LocalDate,
    durations: Map<String, Long>,
): Occurrence? {
    if (rule.days.isNotEmpty() && rule.days.none { it.dayOfWeek == date.dayOfWeek }) return null
    if (schedule.endDate != null && date.isAfter(schedule.endDate)) return null
    val clipMs = durations[rule.action.mediaId] ?: 0L
    val s = rule.start
    val e = rule.end
    val a = rule.at
    return when {
        s != null && e != null && a == null -> {
            val start = resolveLocal(zone, date, s)
            val endDate = if (e.isAfter(s)) date else date.plusDays(1)
            var end = resolveLocal(zone, endDate, e)
            if (!rule.action.isLoop) {
                val clipEnd = start.plusMillis(clipMs)
                if (clipEnd.isBefore(end)) end = clipEnd
            }
            Occurrence(index, start, end)
        }
        s == null && e == null && a != null -> {
            val start = resolveLocal(zone, date, a)
            Occurrence(index, start, start.plusMillis(clipMs))
        }
        else -> null
    }
}

private fun occurrencesNear(
    schedule: Schedule,
    zone: ZoneId,
    around: Instant,
    daysBefore: Long,
    daysAfter: Long,
    durations: Map<String, Long>,
): List<Occurrence> {
    val localDate = around.atZone(zone).toLocalDate()
    val out = ArrayList<Occurrence>()
    for (offset in -daysBefore..daysAfter) {
        val date = localDate.plusDays(offset)
        schedule.rules.forEachIndexed { i, rule ->
            occurrence(schedule, zone, i, rule, date, durations)?.let(out::add)
        }
    }
    return out
}

fun nextBoundary(schedule: Schedule, after: Instant, durations: Map<String, Long>): Instant? {
    val zone = schedule.zoneOrNull() ?: return null
    return occurrencesNear(schedule, zone, after, 1, 8, durations)
        .flatMap { o -> if (schedule.rules[o.ruleIndex].at != null) listOf(o.start) else listOf(o.start, o.end) }
        .filter { it.isAfter(after) }
        .minOrNull()
}

fun overrideExpiry(ov: Override, schedule: Schedule?, durations: Map<String, Long>): Instant? {
    val ends = ArrayList<Instant>()
    val action = ov.action
    if (action is OverrideAction.Play && !action.loop) {
        ends += ov.issuedAt.plusMillis(durations[action.mediaId] ?: 0L)
    }
    if (!ov.hold) {
        ov.maxDurationMs?.let { ends += ov.issuedAt.plusMillis(it) }
        if (schedule != null) nextBoundary(schedule, ov.issuedAt, durations)?.let { ends += it }
    }
    return ends.minOrNull()
}

/** What should be playing at [now]. Pure: call it as often as needed. */
fun evaluate(
    schedule: Schedule?,
    override: Override?,
    durations: Map<String, Long>,
    now: Instant,
    current: PlaybackKey?,
): Desired {
    if (override != null) {
        val expiry = overrideExpiry(override, schedule, durations)
        val expired = expiry != null && !now.isBefore(expiry)
        if (!now.isBefore(override.issuedAt) && !expired) {
            val source = "override:${override.overrideId}"
            return when (val a = override.action) {
                is OverrideAction.Stop -> Desired.Idle(source)
                is OverrideAction.Play -> Desired.Play(source, override.issuedAt, a.mediaId, a.loop, ResumePolicy.RESTART)
            }
        }
    }

    if (schedule == null) return Desired.Idle()
    val zone = schedule.zoneOrNull() ?: return Desired.Idle()

    var best: Occurrence? = null
    var bestPriority = Int.MIN_VALUE
    for (o in occurrencesNear(schedule, zone, now, 1, 0, durations)) {
        if (now.isBefore(o.start) || !now.isBefore(o.end)) continue
        val rule = schedule.rules[o.ruleIndex]
        val source = "rule:${rule.id}"
        if (!rule.action.isLoop && rule.latePolicy == LatePolicy.SKIP) {
            val already = current != null && current.source == source && current.anchor == o.start
            val grace = Duration.ofMillis(rule.lateGraceMs ?: DEFAULT_LATE_GRACE_MS)
            if (!already && Duration.between(o.start, now) > grace) continue
        }
        val priority = rule.priority ?: if (rule.action.isLoop) DEFAULT_LOOP_PRIORITY else DEFAULT_ONE_SHOT_PRIORITY
        val b = best
        val better = b == null ||
            priority > bestPriority ||
            (priority == bestPriority && o.start.isAfter(b.start)) ||
            (priority == bestPriority && o.start == b.start && o.ruleIndex < b.ruleIndex)
        if (better) {
            best = o
            bestPriority = priority
        }
    }

    val o = best ?: return Desired.Idle()
    val rule = schedule.rules[o.ruleIndex]
    return Desired.Play("rule:${rule.id}", o.start, rule.action.mediaId, rule.action.isLoop, rule.resume)
}
