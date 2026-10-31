package dev.scenesync.player

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.provider.Settings
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import dev.scenesync.engine.Desired
import dev.scenesync.engine.evaluate
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Keeps SceneSync running in the background after you leave the player, so
 * it stays connected to the Pi. When something new should be on screen (a
 * schedule starts, a command arrives, or calibration is turned on), it brings
 * the player back to the front.
 *
 * It only brings the player back when the wanted content changes, so you can
 * leave the player to use the projector and it won't keep popping up.
 */
class PlayerService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    /** What the player should show the last time we checked, as a short string. */
    private var lastWanted: String? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        // If this projector won't allow a background service, the player still works.
        if (runCatching { startInForeground() }.onFailure {
                (application as SceneSyncApp).controller.reportError("Background service not allowed: ${it.message}")
            }.isFailure
        ) {
            stopSelf()
            return
        }
        val controller = (application as SceneSyncApp).controller
        scope.launch {
            while (isActive) {
                val wanted = wanted(controller)
                if (wanted != lastWanted) {
                    lastWanted = wanted
                    if (wanted != null && !MainActivity.inFront) bringToFront()
                }
                delay(1_000)
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    /** A description of what should be on screen, or null for black. */
    private fun wanted(controller: Controller): String? {
        val cfg = controller.config.value ?: return null
        if (cfg.calibrate) return "calibrate"
        val durations = cfg.media.mapNotNull { m -> m.durationMs?.let { m.mediaId to it } }.toMap()
        return when (val d = evaluate(cfg.schedule, cfg.override, durations, controller.clock.now(), null)) {
            is Desired.Idle -> null
            is Desired.Play -> "${d.source}@${d.anchor}"
        }
    }

    private fun bringToFront() {
        // Android only lets a background app open a screen if it may "display
        // over other apps" (or is the device owner). Without that this does nothing.
        val launch = Intent(this, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_REORDER_TO_FRONT)
        runCatching { startActivity(launch) }
            .onFailure { (application as SceneSyncApp).controller.reportError("Could not bring the player to the front: ${it.message}") }
    }

    private fun startInForeground() {
        val nm = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, "SceneSync running", NotificationManager.IMPORTANCE_MIN),
            )
        }
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentTitle("SceneSync is running")
            .setContentText("Connected to the Pi in the background. Tap to open the player.")
            .setContentIntent(open)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_MIN)
            .build()
        val type = if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, type)
    }

    companion object {
        private const val CHANNEL = "running"
        private const val NOTIFICATION_ID = 1

        /** Safe to call from the player or after boot; does nothing if already running. */
        fun start(context: Context) {
            runCatching {
                ContextCompat.startForegroundService(context, Intent(context, PlayerService::class.java))
            }
        }

        /** True if the player may open itself from the background. */
        fun canComeBack(context: Context): Boolean = Settings.canDrawOverlays(context) ||
            context.getSystemService(android.app.admin.DevicePolicyManager::class.java)
                ?.isDeviceOwnerApp(context.packageName) == true
    }
}
