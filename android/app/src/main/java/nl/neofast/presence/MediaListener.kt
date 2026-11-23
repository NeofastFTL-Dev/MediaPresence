package nl.neofast.presence

import android.content.ComponentName
import android.media.MediaMetadata
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.service.notification.NotificationListenerService
import org.json.JSONObject

/** Bound by Android itself once notification access is granted, so it keeps running without a separate foreground service. */
class MediaListener : NotificationListenerService() {
    companion object {
        const val VLC = "org.videolan.vlc"
        @Volatile var instance: MediaListener? = null
        @Volatile var lastInfo: TrackInfo? = null
        const val KEEPALIVE_MS = 60_000L
    }
    private val handler = Handler(Looper.getMainLooper())
    private var msm: MediaSessionManager? = null
    private var controller: MediaController? = null
    private var notifTitle = ""
    private var notifText = ""
    private var lastKey = ""
    private var lastSentAt = 0L
    private var lastWasActive = false
    private lateinit var cn: ComponentName

    private val cb = object : MediaController.Callback() {
        override fun onPlaybackStateChanged(state: PlaybackState?) = soon()
        override fun onMetadataChanged(metadata: MediaMetadata?) = soon()
        override fun onSessionDestroyed() { handler.post { pick() } }
    }
    private val sessions = MediaSessionManager.OnActiveSessionsChangedListener { handler.post { pick() } }
    private val keepalive = object : Runnable {
        override fun run() { push(true); handler.postDelayed(this, KEEPALIVE_MS) }
    }
    private val net = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) { Sender.resetLan(); handler.postDelayed({ push(true) }, 3000) }
    }

    override fun onListenerConnected() {
        instance = this
        cn = ComponentName(this, MediaListener::class.java)
        msm = getSystemService(MediaSessionManager::class.java)
        try { msm?.addOnActiveSessionsChangedListener(sessions, cn, handler) } catch (_: SecurityException) {}
        try { getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(net) } catch (_: Exception) {}
        pick()
        handler.postDelayed(keepalive, KEEPALIVE_MS)
    }

    override fun onNotificationPosted(sbn: android.service.notification.StatusBarNotification?) {
        if (sbn == null || sbn.packageName != controller?.packageName) return
        val ex = sbn.notification.extras
        val t = ex.getCharSequence(android.app.Notification.EXTRA_TITLE)?.toString() ?: ""
        val x = ex.getCharSequence(android.app.Notification.EXTRA_TEXT)?.toString() ?: ""
        if (t != notifTitle || x != notifText) { notifTitle = t; notifText = x; push(false) }
    }

    private fun readNotif() {
        try { activeNotifications?.firstOrNull { it.packageName == controller?.packageName }?.let { onNotificationPosted(it) } } catch (_: Exception) {}
    }

    override fun onListenerDisconnected() {
        instance = null
        handler.removeCallbacks(keepalive)
        try { msm?.removeOnActiveSessionsChangedListener(sessions) } catch (_: Exception) {}
        try { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(net) } catch (_: Exception) {}
        controller?.unregisterCallback(cb); controller = null
    }

    private fun allowed(pkg: String) = pkg == VLC || pkg == packageName || Prefs(this).allowOthers

    private val pushSoon = Runnable { push(false) }
    /** Coalesce bursts (metadata + state arrive together) into one send within 250 ms. */
    fun soon() { handler.removeCallbacks(pushSoon); handler.postDelayed(pushSoon, 250) }

    fun pick() {
        val list = try { msm?.getActiveSessions(cn).orEmpty().filter { allowed(it.packageName) } } catch (_: SecurityException) { emptyList() }
        val best = list.firstOrNull { it.playbackState?.state == PlaybackState.STATE_PLAYING } ?: list.firstOrNull()
        if (best?.sessionToken != controller?.sessionToken) {
            controller?.unregisterCallback(cb)
            controller = best
            best?.registerCallback(cb, handler)
            notifTitle = ""; notifText = ""; readNotif()
        }
        push(false)
    }

    /** Sends only when something changed, or (force) as a keepalive while playing. */
    fun push(force: Boolean) {
        val p = Prefs(this)
        val c = controller
        val md = c?.metadata ?: if (c != null) MediaMetadata.Builder().build() else null
        val st = c?.playbackState
        val title = listOf(md?.getString(MediaMetadata.METADATA_KEY_TITLE), md?.getString(MediaMetadata.METADATA_KEY_DISPLAY_TITLE), notifTitle).firstOrNull { Meta.usable(it) } ?: ""
        val active = p.enabled && c != null && md != null && title.isNotEmpty() && st != null &&
            st.state != PlaybackState.STATE_STOPPED && st.state != PlaybackState.STATE_NONE && st.state != PlaybackState.STATE_ERROR
        if (!active) {
            Sender.nowPlaying = if (!p.enabled) "Uitgeschakeld" else "Niets"
            lastInfo = null // otherwise the app keeps showing the last track after playback stopped
            if (lastWasActive) { lastWasActive = false; lastKey = ""; Sender.send(this, "{\"stopped\":true}") }
            return
        }
        val playing = st!!.state == PlaybackState.STATE_PLAYING || st.state == PlaybackState.STATE_BUFFERING
        var pos = st.position
        if (playing && st.lastPositionUpdateTime > 0) pos += ((SystemClock.elapsedRealtime() - st.lastPositionUpdateTime) * st.playbackSpeed).toLong()
        val dur = md!!.getLong(MediaMetadata.METADATA_KEY_DURATION)
        val info = Meta.resolve(this, md, title, dur, notifTitle, notifText) { handler.post { push(false) } }
        val now = System.currentTimeMillis()
        // start-of-track timestamp in 3 s buckets: changes only on seek / track change
        lastInfo = info
        val key = "${info.details}|${info.state}|${p.type}|${info.isVideo}|${p.videoMode}|${p.appId}|${p.btnLabel(1)}${p.btnUrl(1)}${p.btnLabel(2)}${p.btnUrl(2)}|$dur|$playing|${if (playing) (now - pos) / 3000 else 0}"
        Sender.nowPlaying = (if (playing) "▶ " else "⏸ ") + info.details + if (info.state.isNotEmpty()) "\n" + info.state else ""
        if (key == lastKey && !(force && playing)) return
        lastKey = key; lastSentAt = now; lastWasActive = true
        val app = when (c.packageName) { VLC -> "VLC"; packageName -> "MediaPresence test"; else -> c.packageName }
        val j = JSONObject().put("title", info.title).put("artist", info.artist).put("album", info.album)
            .put("details", info.details).put("state", info.state)
            .put("type", if (info.isVideo && p.videoMode == 0) 3 else p.type).put("video", info.isVideo).put("appId", p.appId)
            .put("duration", dur).put("position", pos).put("playing", playing).put("app", app)
        val btns = org.json.JSONArray()
        for (i in 1..2) if (p.btnLabel(i).isNotBlank() && p.btnUrl(i).startsWith("http")) btns.put(JSONObject().put("label", p.btnLabel(i)).put("url", p.btnUrl(i)))
        if (btns.length() > 0) j.put("buttons", btns)
        Sender.send(this, j.toString())
    }
}
