package dev.scenesync.player

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.SystemClock
import android.provider.Settings
import android.view.KeyEvent
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
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView
import dev.scenesync.engine.Desired
import dev.scenesync.engine.PlaybackKey
import dev.scenesync.engine.ResumePolicy
import dev.scenesync.engine.evaluate
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.time.Duration

/**
 * Fullscreen player. Every 250 ms it asks the schedule engine what should be
 * on screen and switches only when the answer changes.
 */
class MainActivity : ComponentActivity() {
    private lateinit var controller: Controller
    private lateinit var player: ExoPlayer
    private lateinit var playerView: PlayerView
    private lateinit var overlay: View
    private lateinit var codeView: TextView
    private lateinit var statusView: TextView
    private lateinit var serverInput: EditText

    private var shown: Desired = Desired.Idle()
    private var current: PlaybackKey? = null
    private var overlayUntil = 0L
    private val backPresses = ArrayDeque<Long>()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        controller = (application as SceneSyncApp).controller
        setContentView(R.layout.activity_main)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        hideSystemBars()

        playerView = findViewById(R.id.player_view)
        overlay = findViewById(R.id.overlay)
        codeView = findViewById(R.id.code)
        statusView = findViewById(R.id.status)
        serverInput = findViewById(R.id.server_input)

        player = ExoPlayer.Builder(this).build()
        playerView.player = player
        player.addListener(object : Player.Listener {
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
                launch { controller.status.collect { renderStatus() } }
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

    override fun onDestroy() {
        player.release()
        super.onDestroy()
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
        val now = controller.clock.now()
        val durations = cfg?.media?.mapNotNull { m -> m.durationMs?.let { m.mediaId to it } }?.toMap() ?: emptyMap()
        val desired = evaluate(cfg?.schedule, cfg?.override, durations, now, current)
        if (desired == shown) return
        show(desired, now, durations)
        shown = desired
        current = desired.key
        controller.nowPlaying.value = desired
    }

    private fun show(desired: Desired, now: java.time.Instant, durations: Map<String, Long>) {
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
                    !desired.looping -> sinceStart
                    desired.resume == ResumePolicy.CONTINUE && duration != null && duration > 0 -> sinceStart % duration
                    else -> 0L
                }
                player.setMediaItem(MediaItem.fromUri(Uri.fromFile(file)), startMs)
                player.repeatMode = if (desired.looping) Player.REPEAT_MODE_ONE else Player.REPEAT_MODE_OFF
                player.prepare()
                player.playWhenReady = true
                playerView.visibility = View.VISIBLE
            }
        }
    }

    private fun showBlack() {
        player.stop()
        player.clearMediaItems()
        playerView.visibility = View.INVISIBLE
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
            is Desired.Idle -> "Showing: black"
            is Desired.Play -> "Showing: ${d.mediaId}${if (d.looping) " (loop)" else ""}"
        }
        statusView.text = if (setupNeeded) s.message else "${s.message}\n$nowText"
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
            // This may be the home app, so BACK must not exit. Press it three
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
