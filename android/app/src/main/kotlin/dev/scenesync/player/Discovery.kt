package dev.scenesync.player

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.coroutines.resume

/** Finds the server on the LAN via mDNS (`_scenesync._tcp`). */
object Discovery {
    private const val SERVICE_TYPE = "_scenesync._tcp."

    @Suppress("DEPRECATION")
    suspend fun find(context: Context, timeoutMs: Long): String? {
        val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        val lock = wifi.createMulticastLock("scenesync-discovery").apply { setReferenceCounted(false); acquire() }
        try {
            return withTimeoutOrNull(timeoutMs) {
                suspendCancellableCoroutine { cont ->
                    var done = false
                    val listener = object : NsdManager.DiscoveryListener {
                        override fun onDiscoveryStarted(serviceType: String) {}
                        override fun onDiscoveryStopped(serviceType: String) {}
                        override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                            if (!done) { done = true; cont.resume(null) }
                        }
                        override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
                        override fun onServiceLost(service: NsdServiceInfo) {}
                        override fun onServiceFound(service: NsdServiceInfo) {
                            nsd.resolveService(service, object : NsdManager.ResolveListener {
                                override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) {}
                                override fun onServiceResolved(info: NsdServiceInfo) {
                                    val host = info.host?.hostAddress ?: return
                                    val h = if (host.contains(':')) "[$host]" else host
                                    synchronized(this@Discovery) {
                                        if (!done) { done = true; cont.resume("http://$h:${info.port}") }
                                    }
                                }
                            })
                        }
                    }
                    nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
                    cont.invokeOnCancellation { runCatching { nsd.stopServiceDiscovery(listener) } }
                }
            }
        } finally {
            lock.release()
        }
    }
}
