package dev.scenesync.player

import android.app.Application

class SceneSyncApp : Application() {
    lateinit var controller: Controller
        private set

    override fun onCreate() {
        super.onCreate()
        controller = Controller(this)
        controller.start()
    }
}
