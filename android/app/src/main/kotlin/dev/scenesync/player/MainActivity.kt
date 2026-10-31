package dev.scenesync.player

import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Matrix
import android.net.Uri
import android.os.Bundle
import android.os.SystemClock
import android.provider.Settings
import android.view.KeyEvent
import android.view.TextureView
import android.view.View
import android.view.WindowManager
import android.view.inputmethod.EditorInfo
import android.widget.EditText
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackParameters
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.VideoSize
import androidx.media3.exoplayer.ExoPlayer
import dev.scenesync.engine.Desired
import dev.scenesync.engine.Mapping
import dev.scenesync.engine.PlaybackKey
import dev.scenesync.engine.ResumePolicy
import dev.scenesync.engine.SyncMath
import dev.scenesync.engine.evaluate
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.time.Duration

/**
 * Fullscreen player. Every 250 ms it asks the schedule engine what should be
 * on screen and switches only when the answer changes.
 */
class MainActivity : ComponentActivity() {
    companion object {
        /** True while the player is on screen; read by [PlayerService]. */
        @Volatile var inFront = false
            private set
    }

    private lateinit var controller: Controller
    private lateinit var player: ExoPlayer
    private lateinit var videoView: TextureView
    private lateinit var calibrationView: CalibrationView
    private lateinit var overlay: View
    private lateinit var codeView: TextView
    private lateinit var statusView: TextView
    private lateinit var serverInput: EditText

    /** What is on screen; null forces the next tick to switch (e.g. after calibration). */
    private var shown: Desired? = null
    private var appliedMapping: Mapping? = null

    // Sync groups: how far ahead to seek to cover the seek itself (learned),
    // and a running average of the error for reports.
    private var seekLeadMs = 150L
    private var lastSeekAt = 0L
    private var avgErrorMs = 0.0
    private var current: PlaybackKey? = null
    private var overlayUntil = 0L
    private val backPresses = ArrayDeque<Long>()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        controller = (application as SceneSyncApp).controller
        PlayerService.start(this)
        setContentView(R.layout.activity_main)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        hideSystemBars()

        videoView = findViewById(R.id.video_view)
        calibrationView = findViewById(R.id.calibration)
        overlay = findViewById(R.id.overlay)
        codeView = findViewById(R.id.code)
        statusView = findViewById(R.id.status)
        serverInput = findViewById(R.id.server_input)
        findViewById<TextView>(R.id.title).text = "SceneSync Player ${BuildConfig.VERSION_NAME}"

        player = ExoPlayer.Builder(this).build()
        player.setVideoTextureView(videoView)
        videoView.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> applyTransform() }
        player.addListener(object : Player.Listener {
            override fun onVideoSizeChanged(videoSize: VideoSize) = applyTransform()

            // Stay black until the new clip's first frame, so the last frame of
            // the previous clip never flashes.
            override fun onRenderedFirstFrame() {
                videoView.visibility = View.VISIBLE
            }

            override fun onPlayerError(error: PlaybackException) {
                controller.reportError("Playback error: ${error.errorCodeName} ${error.message ?: ""}")
            }
        })

        serverInput.setOnEditorActionListener { v, actionId, _ ->
            if (actionId == EditorInfo.IME_ACTION_DONE) {
                val text = v.text.toString().trim()
                if (text.isNotEmpty()) {
                    controller.setServerUrl(if (text.startsWith("http")) text else "http://$text")
                }
                true
            } else {
                false
            }
        }

        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                launch {
                    controller.status.collect {
                        renderStatus()
                        if (it.phase == Controller.Phase.CONNECTED) askToComeBackOnItsOwn()
                    }
                }
                launch {
                    while (true) {
                        tick()
                        renderStatus()
                        delay(250)
                    }
                }
            }
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) hideSystemBars()
    }

    override fun onStart() {
        super.onStart()
        inFront = true
    }

    override fun onStop() {
        // Left the player (Back, Home or another app): stop the video and its
        // sound. The background service stays connected to the Pi.
        inFront = false
        showBlack()
        shown = null
        current = null
        calibrationView.visibility = View.GONE
        controller.nowPlaying.value = Desired.Idle("player closed")
        super.onStop()
    }

    override fun onDestroy() {
        player.release()
        super.onDestroy()
    }

    /**
     * Once, after pairing: open the "Display over other apps" setting. With it
     * on, the background service can bring the player back when a schedule
     * starts or a command arrives.
     */
    private fun askToComeBackOnItsOwn() {
        val prefs = Prefs(this)
        if (prefs.askedOverlay || PlayerService.canComeBack(this)) return
        prefs.askedOverlay = true
        val intent = Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:$packageName"))
        runCatching { startActivity(intent) }
            .onFailure { controller.reportError("No 'Display over other apps' setting on this projector") }
    }

    private fun isHomeApp(): Boolean {
        val home = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME)
        return packageManager.resolveActivity(home, PackageManager.MATCH_DEFAULT_ONLY)
            ?.activityInfo?.packageName == packageName
    }

    private fun hideSystemBars() {
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    private fun tick() {
        val cfg = controller.config.value
        if (cfg?.mapping != appliedMapping) {
            appliedMapping = cfg?.mapping
            applyTransform()
        }
        if (cfg?.calibrate == true) {
            calibrationView.mapping = cfg.mapping
            calibrationView.label = cfg.name
            if (calibrationView.visibility != View.VISIBLE) {
                calibrationView.visibility = View.VISIBLE
                showBlack()
                shown = null
                current = null
                controller.nowPlaying.value = Desired.Idle("calibration")
            }
            return
        }
        calibrationView.visibility = View.GONE

        val now = controller.clock.now()
        val durations = cfg?.media?.mapNotNull { m -> m.durationMs?.let { m.mediaId to it } }?.toMap() ?: emptyMap()
        val desired = evaluate(cfg?.schedule, cfg?.override, durations, now, current)
        val volume = if (cfg?.sync?.muted == true) 0f else 1f
        if (player.volume != volume) player.volume = volume
        if (desired == shown) {
            if (desired is Desired.Play && cfg?.sync != null) keepInSync(desired, cfg.sync!!.trimMs, durations)
            return
        }
        if (cfg?.sync == null) controller.syncErrorMs = null
        show(desired, now, durations, cfg?.sync?.trimMs)
        shown = desired
        current = desired.key
        controller.nowPlaying.value = desired
    }

    /**
     * Maps the video onto the screen. TextureView stretches the video over the
     * whole view, so the transform takes the view's corners to either the
     * letterboxed rectangle (no mapping) or the four fit-to-window corners.
     */
    private fun applyTransform() {
        val w = videoView.width.toFloat()
        val h = videoView.height.toFloat()
        if (w <= 0f || h <= 0f) return
        val dst = appliedMapping?.corners?.takeIf { it.size == 4 && it.all { p -> p.size == 2 } }
            ?.flatMap { p -> listOf((p[0] * w).toFloat(), (p[1] * h).toFloat()) }
            ?.toFloatArray()
            ?: letterbox(w, h)
        val src = floatArrayOf(0f, 0f, w, 0f, w, h, 0f, h)
        val m = Matrix()
        if (!m.setPolyToPoly(src, 0, dst, 0, 4)) m.reset()
        videoView.setTransform(m)
        videoView.invalidate()
    }

    private fun letterbox(w: Float, h: Float): FloatArray {
        val v = player.videoSize
        if (v.width <= 0 || v.height <= 0) return floatArrayOf(0f, 0f, w, 0f, w, h, 0f, h)
        val aspect = v.width * v.pixelWidthHeightRatio / v.height
        val (bw, bh) = if (w / h > aspect) Pair(h * aspect, h) else Pair(w, w / aspect)
        val x = (w - bw) / 2
        val y = (h - bh) / 2
        return floatArrayOf(x, y, x + bw, y, x + bw, y + bh, x, y + bh)
    }

    /**
     * In a sync group every projector plays against the same anchor on the
     * server clock. Compare where the clip is with where it should be, then
     * nudge the speed or seek.
     */
    private fun keepInSync(d: Desired.Play, trimMs: Long, durations: Map<String, Long>) {
        if (player.playbackState != Player.STATE_READY || !player.isPlaying) return
        val duration = durations[d.mediaId] ?: player.duration.takeIf { it > 0 }
        val expected = SyncMath.expectedPositionMs(
            d.anchor.toEpochMilli(), controller.clock.now().toEpochMilli(), duration, d.looping, trimMs,
        ) ?: return
        val error = SyncMath.errorMs(player.currentPosition, expected, duration, d.looping)
        val sinceSeek = SystemClock.elapsedRealtime() - lastSeekAt
        if (sinceSeek in 1..3_000) {
            // Right after a seek, the remaining error says how far off the lead was.
            if (sinceSeek > 500) {
                seekLeadMs = (seekLeadMs - error / 2).coerceIn(0L, 1_500L)
                lastSeekAt = 0L
            }
            return
        }
        avgErrorMs = avgErrorMs * 0.9 + kotlin.math.abs(error) * 0.1
        controller.syncErrorMs = avgErrorMs.toLong()
        when (val c = SyncMath.correction(error, expected, seekLeadMs, duration, d.looping)) {
            is SyncMath.Correction.Speed ->
                if (player.playbackParameters.speed != c.speed) player.playbackParameters = PlaybackParameters(c.speed)
            is SyncMath.Correction.Seek -> {
                player.playbackParameters = PlaybackParameters(1f)
                player.seekTo(c.toMs)
                lastSeekAt = SystemClock.elapsedRealtime()
            }
        }
    }

    private fun show(desired: Desired, now: java.time.Instant, durations: Map<String, Long>, syncTrimMs: Long? = null) {
        when (desired) {
            is Desired.Idle -> showBlack()
            is Desired.Play -> {
                val file = controller.mediaFile(desired.mediaId)
                if (file == null) {
                    controller.reportError("Clip ${desired.mediaId} is not downloaded")
                    showBlack()
                    return
                }
                val sinceStart = Duration.between(desired.anchor, now).toMillis().coerceAtLeast(0)
                val duration = durations[desired.mediaId]
                val startMs = when {
                    // In a sync group, loops always follow the shared timeline.
                    syncTrimMs != null -> (SyncMath.expectedPositionMs(
                        desired.anchor.toEpochMilli(), now.toEpochMilli(), duration, desired.looping, syncTrimMs,
                    ) ?: 0L) + seekLeadMs
                    !desired.looping -> sinceStart
                    desired.resume == ResumePolicy.CONTINUE && duration != null && duration > 0 -> sinceStart % duration
                    else -> 0L
                }
                videoView.visibility = View.INVISIBLE
                player.playbackParameters = PlaybackParameters(1f)
                player.setMediaItem(MediaItem.fromUri(Uri.fromFile(file)), startMs)
                player.repeatMode = if (desired.looping) Player.REPEAT_MODE_ONE else Player.REPEAT_MODE_OFF
                player.prepare()
                player.playWhenReady = true
            }
        }
    }

    private fun showBlack() {
        player.stop()
        player.clearMediaItems()
        videoView.visibility = View.INVISIBLE
    }

    private fun renderStatus() {
        val s = controller.status.value
        val setupNeeded = s.phase == Controller.Phase.FINDING_SERVER ||
            s.phase == Controller.Phase.NEED_SERVER_URL ||
            s.phase == Controller.Phase.PAIRING ||
            controller.config.value == null
        val visible = setupNeeded || SystemClock.elapsedRealtime() < overlayUntil
        overlay.visibility = if (visible) View.VISIBLE else View.GONE
        codeView.visibility = if (s.code != null) View.VISIBLE else View.GONE
        codeView.text = s.code ?: ""
        val nowText = when (val d = shown) {
            null -> if (controller.config.value?.calibrate == true) "Showing: calibration pattern" else "Showing: black"
            is Desired.Idle -> "Showing: black"
            is Desired.Play -> "Showing: ${d.mediaId}${if (d.looping) " (loop)" else ""}"
        }
        val tip = if (!setupNeeded && !PlayerService.canComeBack(this)) {
            "\nTip: turn on \"Display over other apps\" for SceneSync so it can reopen itself."
        } else {
            ""
        }
        statusView.text = if (setupNeeded) s.message else "${s.message}\n$nowText$tip"
        val wantInput = s.phase == Controller.Phase.NEED_SERVER_URL
        if (wantInput && serverInput.visibility != View.VISIBLE) {
            serverInput.visibility = View.VISIBLE
            serverInput.requestFocus()
        } else if (!wantInput) {
            serverInput.visibility = View.GONE
        }
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (keyCode == KeyEvent.KEYCODE_BACK) {
            // Back (or a right-click) leaves the player. The service keeps
            // running and brings it back when something new should play.
            if (!isHomeApp()) {
                moveTaskToBack(true)
                return true
            }
            // As the home app there is nothing to go back to. Press Back three
            // times within two seconds to open system settings.
            val t = SystemClock.elapsedRealtime()
            backPresses.addLast(t)
            while (backPresses.isNotEmpty() && t - backPresses.first() > 2_000) backPresses.removeFirst()
            if (backPresses.size >= 3) {
                backPresses.clear()
                runCatching { startActivity(Intent(Settings.ACTION_SETTINGS)) }
            }
            overlayUntil = t + 10_000
            return true
        }
        // Any other key shows the status overlay for 10 seconds.
        overlayUntil = SystemClock.elapsedRealtime() + 10_000
        return super.onKeyDown(keyCode, event)
    }
}
