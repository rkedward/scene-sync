package dev.scenesync.player

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * After boot or an update: starts the background service, then tries to open
 * the player. Android 10+ may block opening it unless "Display over other
 * apps" is on; setting SceneSync as the home app always works.
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        PlayerService.start(context)
        val launch = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(launch) }
    }
}
