package nl.neofast.presence

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.widget.*
import com.google.zxing.integration.android.IntentIntegrator

class MainActivity : Activity() {
    private val h = Handler(Looper.getMainLooper())
    private lateinit var access: TextView
    private lateinit var pairState: TextView
    private lateinit var playing: TextView
    private lateinit var sent: TextView
    private lateinit var accessBtn: Button
    private lateinit var mediaBtn: Button
    private lateinit var preview: TextView
    private lateinit var diagView: TextView
    private var lastRebind = 0L
    private var testSession: MediaSession? = null
    private val orange = Color.rgb(255, 136, 0)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val p = Prefs(this)
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(48, 96, 48, 48); setBackgroundColor(Color.rgb(18, 18, 24)) }
        fun tv(s: String, size: Float = 16f, color: Int = Color.rgb(230, 230, 240), bold: Boolean = false) = TextView(this).apply {
            text = s; textSize = size; setTextColor(color); setPadding(0, 12, 0, 12); if (bold) typeface = Typeface.DEFAULT_BOLD
        }.also { root.addView(it) }
        fun btn(s: String, f: () -> Unit) = Button(this).apply { text = s; setOnClickListener { f() } }.also { root.addView(it) }
        tv("MediaPresence", 30f, orange, true)
        tv("Laat zien wat je in VLC luistert op je Discord-profiel.", 15f)
        btn("QR-code van je pc scannen") { scanQr() }
        access = tv(""); accessBtn = btn("Meldingstoegang geven") {
            startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
        }
        pairState = tv("")
        val code = EditText(this).apply { hint = "Of plak hier de koppelcode van je pc"; setTextColor(Color.WHITE); setHintTextColor(Color.GRAY); isSingleLine = true }
        root.addView(code)
        btn("Koppelen") { pair(code.text.toString()); code.setText("") }
        root.addView(Switch(this).apply {
            text = "Status op Discord tonen"; setTextColor(Color.WHITE); isChecked = p.enabled; setPadding(0, 24, 0, 12)
            setOnCheckedChangeListener { _, v -> p.enabled = v; MediaListener.instance?.push(false) }
        })
        root.addView(Switch(this).apply {
            text = "Ook andere muziekapps (niet alleen VLC)"; setTextColor(Color.WHITE); isChecked = p.allowOthers; setPadding(0, 12, 0, 12)
            setOnCheckedChangeListener { _, v -> p.allowOthers = v; MediaListener.instance?.pick() }
        })
        tv("Weergave", 18f, orange, true)
        mediaBtn = btn("Toegang tot muziekbestanden geven (voor tags)") {
            requestPermissions(if (android.os.Build.VERSION.SDK_INT >= 33) arrayOf(android.Manifest.permission.READ_MEDIA_AUDIO, android.Manifest.permission.READ_MEDIA_VIDEO) else arrayOf(android.Manifest.permission.READ_EXTERNAL_STORAGE), 1)
        }
        tv("Bron van de titel", 14f, Color.LTGRAY)
        root.addView(Spinner(this).apply {
            adapter = ArrayAdapter(this@MainActivity, android.R.layout.simple_spinner_dropdown_item,
                listOf("Automatisch (tags, anders bestandsnaam)", "Altijd bestandsnaam", "Alleen tags"))
            setSelection(p.source)
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(a: AdapterView<*>?, v: View?, pos: Int, id: Long) { if (p.source != pos) { p.source = pos; MediaListener.instance?.push(false) } }
                override fun onNothingSelected(a: AdapterView<*>?) {}
            }
        })
        root.addView(Switch(this).apply {
            text = "Bestandsnaam zonder extensie (.mp3)"; setTextColor(Color.WHITE); isChecked = p.stripExt; setPadding(0, 12, 0, 12)
            setOnCheckedChangeListener { _, v -> p.stripExt = v; MediaListener.instance?.push(false) }
        })
        tv("Placeholders: {title}/{titel}, {artist}/{artiest}, {album}, {year}/{jaar}, {filename}/{bestandsnaam}, {type}/{soort}", 13f, Color.LTGRAY)
        fun field(label: String, value: String, save: (String) -> Unit) {
            tv(label, 14f, Color.LTGRAY)
            root.addView(EditText(this).apply {
                setText(value); setTextColor(Color.WHITE); setHintTextColor(Color.GRAY); isSingleLine = true
                hint = "leeg = regel verbergen"
                setOnFocusChangeListener { _, f -> if (!f) { save(text.toString()); MediaListener.instance?.push(false) } }
                setOnEditorActionListener { v, _, _ -> save(v.text.toString()); MediaListener.instance?.push(false); false }
            })
        }
        field("Regel 1 (bovenste regel)", p.detailsTpl) { p.detailsTpl = it.ifBlank { "{title}" } }
        field("Regel 2", p.stateTpl) { p.stateTpl = it }
        field("Tekst als de artiest onbekend is", p.noArtist) { p.noArtist = it }
        tv("Discord-profiel", 18f, orange, true)
        val profSpin = Spinner(this); root.addView(profSpin)
        fun loadProfiles() {
            profSpin.adapter = ArrayAdapter(this, android.R.layout.simple_spinner_dropdown_item, p.profiles.map { "${it.first} (${it.second})" })
            profSpin.setSelection(p.profile.coerceIn(0, p.profiles.size - 1))
        }
        loadProfiles()
        profSpin.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onItemSelected(a: AdapterView<*>?, v: View?, pos: Int, id: Long) { if (p.profile != pos) { p.profile = pos; MediaListener.instance?.push(true) } }
            override fun onNothingSelected(a: AdapterView<*>?) {}
        }
        val pName = EditText(this).apply { hint = "Naam (zoals in Discord)"; setTextColor(Color.WHITE); setHintTextColor(Color.GRAY); isSingleLine = true }
        val pId = EditText(this).apply { hint = "Application ID"; setTextColor(Color.WHITE); setHintTextColor(Color.GRAY); isSingleLine = true; inputType = android.text.InputType.TYPE_CLASS_NUMBER }
        root.addView(pName); root.addView(pId)
        btn("Profiel toevoegen") {
            val id = pId.text.toString().trim()
            if (id.length < 15 || !id.all(Char::isDigit)) { Toast.makeText(this, "Application ID = alleen cijfers (15-20)", Toast.LENGTH_LONG).show(); return@btn }
            p.profiles = p.profiles + (pName.text.toString().ifBlank { "Profiel" } to id); p.profile = p.profiles.size - 1
            pName.setText(""); pId.setText(""); loadProfiles(); MediaListener.instance?.push(true)
        }
        btn("Gekozen profiel verwijderen") {
            if (p.profiles.size <= 1) return@btn
            p.profiles = p.profiles.filterIndexed { i, _ -> i != p.profile }; p.profile = 0; loadProfiles(); MediaListener.instance?.push(true)
        }
        tv("Soort activiteit", 14f, Color.LTGRAY)
        val types = listOf(2, 0, 3)
        root.addView(Spinner(this).apply {
            adapter = ArrayAdapter(this@MainActivity, android.R.layout.simple_spinner_dropdown_item, listOf("Luistert naar …", "Speelt …", "Kijkt naar …"))
            setSelection(types.indexOf(p.type).coerceAtLeast(0))
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(a: AdapterView<*>?, v: View?, pos: Int, id: Long) { if (p.type != types[pos]) { p.type = types[pos]; MediaListener.instance?.push(true) } }
                override fun onNothingSelected(a: AdapterView<*>?) {}
            }
        })
        root.addView(Switch(this).apply {
            text = "Video's automatisch als \"Kijkt naar\""; setTextColor(Color.WHITE); isChecked = p.videoMode == 0; setPadding(0, 12, 0, 12)
            setOnCheckedChangeListener { _, v -> p.videoMode = if (v) 0 else 1; MediaListener.instance?.push(true) }
        })
        tv("Knoppen (max 2, zichtbaar voor anderen)", 14f, Color.LTGRAY)
        for (i in 1..2) {
            val l = EditText(this).apply { hint = "Knop $i tekst"; setText(p.btnLabel(i)); setTextColor(Color.WHITE); setHintTextColor(Color.GRAY); isSingleLine = true }
            val u = EditText(this).apply { hint = "Knop $i link (https://…)"; setText(p.btnUrl(i)); setTextColor(Color.WHITE); setHintTextColor(Color.GRAY); isSingleLine = true }
            val save = { p.setBtn(i, l.text.toString(), u.text.toString()); MediaListener.instance?.push(true) }
            l.setOnFocusChangeListener { _, f -> if (!f) save() }; u.setOnFocusChangeListener { _, f -> if (!f) save() }
            root.addView(l); root.addView(u)
        }
        tv("Voorbeeld", 18f, orange, true)
        preview = TextView(this).apply { setBackgroundColor(Color.rgb(43, 45, 49)); setTextColor(Color.WHITE); setPadding(36, 28, 36, 28); textSize = 15f }
        root.addView(preview)
        playing = tv("", 17f, Color.WHITE, true)
        sent = tv("", 14f, Color.LTGRAY)
        btn("Test-nummer afspelen / stoppen") { toggleTest() }
        diagView = tv("", 12f, Color.LTGRAY).apply { visibility = View.GONE; setTextIsSelectable(true) }
        btn("Diagnose tonen/verbergen") { diagView.visibility = if (diagView.visibility == View.GONE) View.VISIBLE else View.GONE }
        setContentView(ScrollView(this).apply { addView(root); setBackgroundColor(Color.rgb(18, 18, 24)) })
        handleIntent(intent)
    }

    override fun onNewIntent(intent: Intent) { super.onNewIntent(intent); handleIntent(intent) }

    private fun handleIntent(i: Intent?) { if (i == null) return; setIntent(i); i.data?.let { pair(it.toString()) } }

    /** Opens the camera scanner; the result comes back in onActivityResult. */
    private fun scanQr() {
        try {
            IntentIntegrator(this)
                .setDesiredBarcodeFormats(IntentIntegrator.QR_CODE)
                .setPrompt("Richt je camera op de QR-code in MediaPresence op je pc")
                .setBeepEnabled(false)
                .setOrientationLocked(false)
                .initiateScan()
        } catch (e: Exception) {
            Toast.makeText(this, "Scanner start niet: ${e.message}", Toast.LENGTH_LONG).show()
        }
    }

    @Deprecated("Activity result API is not available on a plain Activity")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        val result = try { IntentIntegrator.parseActivityResult(requestCode, resultCode, data) } catch (_: Exception) { null }
        if (result != null) {
            val text = result.contents
            if (text.isNullOrBlank()) {
                if (resultCode != RESULT_CANCELED) Toast.makeText(this, "Geen QR-code gevonden", Toast.LENGTH_LONG).show()
            } else pair(text)
            return
        }
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
    }

    private fun pair(text: String) {
        if (Prefs(this).applyPairUri(text)) {
            Toast.makeText(this, "Gekoppeld met je pc!", Toast.LENGTH_SHORT).show()
            MediaListener.instance?.push(true)
        } else if (text.isNotBlank()) Toast.makeText(this, "Dat is geen geldige koppelcode", Toast.LENGTH_LONG).show()
        refresh()
    }

    private fun hasAccess(): Boolean {
        val flat = Settings.Secure.getString(contentResolver, "enabled_notification_listeners") ?: return false
        return flat.split(':').any { ComponentName.unflattenFromString(it)?.packageName == packageName }
    }

    private fun refresh() {
        val ok = hasAccess()
        access.text = if (ok) "✔ Meldingstoegang staat aan" else "✖ Geef MediaPresence meldingstoegang, zodat hij kan zien wat VLC speelt."
        access.setTextColor(if (ok) Color.rgb(120, 220, 120) else Color.rgb(255, 120, 120))
        accessBtn.visibility = if (ok) View.GONE else View.VISIBLE
        mediaBtn.visibility = if ((android.os.Build.VERSION.SDK_INT < 33 || checkSelfPermission(android.Manifest.permission.READ_MEDIA_VIDEO) == android.content.pm.PackageManager.PERMISSION_GRANTED) && checkSelfPermission(if (android.os.Build.VERSION.SDK_INT >= 33) android.Manifest.permission.READ_MEDIA_AUDIO else android.Manifest.permission.READ_EXTERNAL_STORAGE) == android.content.pm.PackageManager.PERMISSION_GRANTED) View.GONE else View.VISIBLE
        val p = Prefs(this)
        pairState.text = if (p.token.isEmpty()) "✖ Nog niet gekoppeld: tik hierboven op \"QR-code van je pc scannen\" of plak de koppelcode."
            else "✔ Gekoppeld" + (if (p.lan.isNotEmpty()) " (wifi: ${p.lan})" else "") + "\nOnderweg via ${p.pub}"
        playing.text = "Nu: " + Sender.nowPlaying
        sent.text = Sender.status
        if (diagView.visibility == View.VISIBLE) diagView.text = "Versie ${packageManager.getPackageInfo(packageName, 0).versionName}\n" + Meta.diag
        if (ok && MediaListener.instance == null && android.os.SystemClock.elapsedRealtime() - lastRebind > 10_000) {
            lastRebind = android.os.SystemClock.elapsedRealtime()
            android.service.notification.NotificationListenerService.requestRebind(ComponentName(this, MediaListener::class.java))
        }
        val inf = MediaListener.lastInfo
        val prof = p.profiles.getOrElse(p.profile) { p.profiles[0] }.first
        val verb = when (if (inf?.isVideo == true && p.videoMode == 0) 3 else p.type) { 0 -> "Speelt"; 3 -> "Kijkt naar"; else -> "Luistert naar" }
        val btnTxt = (1..2).filter { p.btnLabel(it).isNotBlank() && p.btnUrl(it).startsWith("http") }.joinToString("") { "\n[ ${p.btnLabel(it)} ]" }
        preview.text = if (inf == null) "$verb $prof\n(speel iets af om een voorbeeld te zien)$btnTxt"
            else "$verb $prof\n${inf.details}" + (if (inf.state.isNotEmpty()) "\n${inf.state}" else "") + btnTxt
    }

    private val tick = object : Runnable { override fun run() { refresh(); h.postDelayed(this, 1000) } }
    override fun onResume() { super.onResume(); h.removeCallbacks(tick); h.post(tick) }
    override fun onPause() { super.onPause(); h.removeCallbacks(tick); currentFocus?.clearFocus() }

    /** Fake media session so you can test without VLC. */
    private fun toggleTest() {
        val s = testSession
        if (s != null) {
            s.setPlaybackState(PlaybackState.Builder().setState(PlaybackState.STATE_STOPPED, 0, 0f).build())
            s.isActive = false; s.release(); testSession = null
            Toast.makeText(this, "Test gestopt", Toast.LENGTH_SHORT).show(); return
        }
        val m = MediaSession(this, "MediaPresenceTest")
        m.setMetadata(MediaMetadata.Builder()
            .putString(MediaMetadata.METADATA_KEY_TITLE, "neopresence_test.mp3")
            .putLong(MediaMetadata.METADATA_KEY_DURATION, 60_000).build())
        m.setPlaybackState(PlaybackState.Builder()
            .setActions(PlaybackState.ACTION_PLAY_PAUSE)
            .setState(PlaybackState.STATE_PLAYING, 30_000, 1f).build())
        m.isActive = true
        testSession = m
        Toast.makeText(this, "Test-nummer speelt (3 min)", Toast.LENGTH_SHORT).show()
    }

    override fun onDestroy() { testSession?.release(); testSession = null; super.onDestroy() }
}
