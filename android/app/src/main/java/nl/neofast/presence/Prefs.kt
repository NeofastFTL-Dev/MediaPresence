package nl.neofast.presence

import android.content.Context

class Prefs(ctx: Context) {
    private val p = ctx.getSharedPreferences("neopresence", Context.MODE_PRIVATE)

    private companion object {
        /** host:port, so a malformed pairing link can never produce "http:///update". */
        val LAN_RE = Regex("""^[A-Za-z0-9.\-]+:\d{2,5}$""")
    }
    var token: String get() = p.getString("token", "")!!; set(v) = p.edit().putString("token", v).apply()
    var lan: String get() = p.getString("lan", "")!!; set(v) = p.edit().putString("lan", v).apply()
    var pub: String get() = p.getString("pub", "https://presence.neofastftldev.nl")!!; set(v) = p.edit().putString("pub", v).apply()
    var enabled: Boolean get() = p.getBoolean("enabled", true); set(v) = p.edit().putBoolean("enabled", v).apply()
    /** 0 = automatisch (tags, anders bestandsnaam), 1 = altijd bestandsnaam, 2 = alleen tags */
    var source: Int get() = p.getInt("source", 0); set(v) = p.edit().putInt("source", v).apply()
    var stripExt: Boolean get() = p.getBoolean("stripExt", true); set(v) = p.edit().putBoolean("stripExt", v).apply()
    var detailsTpl: String get() = p.getString("detailsTpl", "{title}")!!; set(v) = p.edit().putString("detailsTpl", v).apply()
    var stateTpl: String get() = p.getString("stateTpl", "🎤 {artist}")!!.let { if (it == "door {artist}") "🎤 {artist}" else it }; set(v) = p.edit().putString("stateTpl", v).apply()
    /** Text used when the artist is unknown; empty = hide that line. */
    var noArtist: String get() = p.getString("noArtist", "")!!; set(v) = p.edit().putString("noArtist", v).apply()
    /** Profiles as lines "naam|appId". */
    var profiles: List<Pair<String, String>>
        get() = p.getString("profiles", "VLC|1562742366731763765")!!.lines().mapNotNull { l -> l.split('|').takeIf { it.size == 2 && it[1].all(Char::isDigit) && it[1].length >= 15 }?.let { it[0] to it[1] } }
            .ifEmpty { listOf("VLC" to "1562742366731763765") }
        set(v) = p.edit().putString("profiles", v.joinToString("\n") { "${it.first.replace("|", "/")}|${it.second}" }).apply()
    var profile: Int get() = p.getInt("profile", 0); set(v) = p.edit().putInt("profile", v).apply()
    val appId get() = profiles.getOrElse(profile) { profiles[0] }.second
    /** Discord activity type: 2 Luistert naar, 0 Speelt, 3 Kijkt naar */
    var type: Int get() = p.getInt("type", 2); set(v) = p.edit().putInt("type", v).apply()
    fun btnLabel(i: Int) = p.getString("b${i}l", "")!!
    fun btnUrl(i: Int) = p.getString("b${i}u", "")!!
    fun setBtn(i: Int, l: String, u: String) = p.edit().putString("b${i}l", l.take(32)).putString("b${i}u", u.trim()).apply()
    /** 0 = automatisch: video -> "Kijkt naar", 1 = altijd het gekozen type */
    var videoMode: Int get() = p.getInt("videoMode", 0); set(v) = p.edit().putInt("videoMode", v).apply()
    var allowOthers: Boolean get() = p.getBoolean("others", false); set(v) = p.edit().putBoolean("others", v).apply()

    /**
     * Accepts both the scanned pairing link (neopresence://pair?t=..&lan=..&pub=..) and the
     * bare pairing code the PC's "Kopieer koppelcode" button puts on the clipboard.
     * Returns true when a usable token was found.
     */
    fun applyPairUri(uriText: String): Boolean {
        val raw = uriText.trim()
        if (raw.isEmpty()) return false
        val u = android.net.Uri.parse(raw)
        if (u.scheme != "neopresence") {
            // A pasted bare code: 32+ letters/digits and no link punctuation.
            if (raw.length >= 32 && raw.all { it.isLetterOrDigit() }) { token = raw; return true }
            return false
        }
        val t = u.getQueryParameter("t") ?: return false
        if (t.length < 32) return false
        token = t
        u.getQueryParameter("lan")?.let { if (LAN_RE.matches(it.trim())) lan = it.trim() }
        u.getQueryParameter("pub")?.let { if (it.startsWith("https://")) pub = it.trimEnd('/') }
        return true
    }
}
