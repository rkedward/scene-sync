package dev.scenesync.player

import android.app.admin.DeviceAdminReceiver

/** Lets SceneSync be made device owner (optional), for prompt-free updates. */
class AdminReceiver : DeviceAdminReceiver()
