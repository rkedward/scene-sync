package dev.scenesync.player

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.coroutines.resume

/** Finds the server on the LAN via mDNS (`_scenesync._tcp`). */
object Discovery {
    private const val SERVICE_TYPE = "_scenesync._tcp."

    /**
     * An address an HTTP client can use: IPv4 first, then a global IPv6.
     * Link-local IPv6 (fe80::…%wlan0) is skipped: OkHttp can't use it.
     */
    @Suppress("DEPRECATION")
    private fun usableHost(info: NsdServiceInfo): String? {
        val all: List<InetAddress> =
            if (Build.VERSION.SDK_INT >= 34) info.hostAddresses else listOfNotNull(info.host)
        all.firstOrNull { it is Inet4Address }?.let { return it.hostAddress }
        val v6 = all.firstOrNull { it is Inet6Address && !it.isLinkLocalAddress && !it.isSiteLocalAddress }
            ?: return null
        return "[" + v6.hostAddress!!.substringBefore('%') + "]"
    }

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
                                    val h = usableHost(info) ?: return
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
