package dev.scenesync.player

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Tries to bring the player up after boot. Android 10+ may block this for a
 * normal app; setting SceneSync as the home app is the reliable path.
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val launch = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(launch) }
    }
}
