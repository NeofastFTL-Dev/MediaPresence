package nl.neofast.presence

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.URL
import java.util.concurrent.Executors

object Sender {
    private val ex = Executors.newSingleThreadExecutor()
    @Volatile var status = "Nog niets verstuurd"
    @Volatile var nowPlaying = "Niets"
    private val pending = java.util.concurrent.atomic.AtomicReference<String?>(null)
    @Volatile private var lanBackoffUntil = 0L

    /** Only the newest payload matters: older queued ones are dropped. */
    fun resetLan() { lanBackoffUntil = 0 }
    fun send(ctx: Context, json: String) {
        val app = ctx.applicationContext
        pending.set(json)
        ex.execute {
            val j = pending.getAndSet(null)
            if (j != null) try { doSend(app, j) } catch (e: Exception) { status = "Fout: ${e.message}" }
        }
    }

    private fun onWifi(ctx: Context): Boolean {
        val cm = ctx.getSystemService(ConnectivityManager::class.java)
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) || caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
    }

    private fun doSend(ctx: Context, json: String) {
        val p = Prefs(ctx)
        if (p.token.isEmpty()) { status = "Niet gekoppeld: scan de QR-code op je pc"; return }
        if (onWifi(ctx) && System.currentTimeMillis() > lanBackoffUntil) {
            if (p.lan.isNotEmpty() && post("http://${p.lan}/update", json, p.token, 1500) == 200) { status = "Verstuurd via wifi (${time()})"; return }
            val found = discover()
            if (found != null) {
                p.lan = found
                if (post("http://$found/update", json, p.token, 1500) == 200) { status = "Verstuurd via wifi (${time()})"; return }
            }
            lanBackoffUntil = System.currentTimeMillis() + 10 * 60_000 // LAN blocked (e.g. firewall): use internet for 10 min
        }
        val t0 = System.currentTimeMillis()
        val code = post(p.pub + "/update", json, p.token, 10000)
        val ms = System.currentTimeMillis() - t0
        status = when (code) {
            200 -> "Verstuurd via internet (${time()}, $ms ms)"
            401 -> "Token klopt niet: koppel opnieuw"
            429 -> "Te veel verzoeken, even wachten"
            else -> "PC niet bereikbaar ($code). Staat NeoPresence aan op je pc?"
        }
    }

    private fun time() = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.ROOT).format(java.util.Date())

    private fun post(url: String, json: String, token: String, timeout: Int): Int = try {
        val c = URL(url).openConnection() as HttpURLConnection
        c.connectTimeout = timeout; c.readTimeout = timeout
        c.requestMethod = "POST"; c.doOutput = true
        c.setRequestProperty("Authorization", "Bearer $token")
        c.setRequestProperty("Content-Type", "application/json")
        val b = json.toByteArray(Charsets.UTF_8)
        c.setFixedLengthStreamingMode(b.size)
        c.outputStream.use { it.write(b) }
        val code = c.responseCode
        try { (if (code < 400) c.inputStream else c.errorStream)?.close() } catch (_: Exception) {}
        c.disconnect(); code
    } catch (e: Exception) { -1 }

    /** UDP broadcast on port 47801; the PC answers "NEOPRESENCE:<port>". */
    private fun discover(): String? = try {
        DatagramSocket().use { s ->
            s.broadcast = true; s.soTimeout = 1500
            val msg = "NEOPRESENCE_DISCOVER".toByteArray()
            s.send(DatagramPacket(msg, msg.size, InetAddress.getByName("255.255.255.255"), 47801))
            val buf = ByteArray(64); val pk = DatagramPacket(buf, buf.size)
            s.receive(pk)
            val txt = String(pk.data, 0, pk.length)
            if (txt.startsWith("NEOPRESENCE:")) "${pk.address.hostAddress}:${txt.substringAfter(':')}" else null
        }
    } catch (e: Exception) { null }
}
