package nl.neofast.presence

import android.content.ContentUris
import android.content.Context
import android.media.MediaMetadata
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.provider.MediaStore
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

data class TrackInfo(val isVideo: Boolean, val title: String, val artist: String, val album: String, val year: String,
                     val filename: String, val details: String, val state: String)

/** Combines media-session metadata with the file's own tags (ID3/MP4/FLAC/OGG via MediaMetadataRetriever). */
object Meta {
    private data class FileTags(val title: String, val artist: String, val album: String, val year: String, val filename: String, val video: Boolean = false)
    private val cache = ConcurrentHashMap<String, FileTags>()
    private val lastUri = ConcurrentHashMap<String, String>()
    private val busy = ConcurrentHashMap.newKeySet<String>()
    private val ex = Executors.newSingleThreadExecutor()
    private val extRe = Regex("""\.(mp3|m4a|mp4|aac|flac|ogg|oga|opus|wav|wma|aiff?|ape|mka|mkv|webm|alac|mid|midi|amr|3gp|avi|mov|m4v|wmv|flv|ts|m2ts|mpg|mpeg|vob|ogv)$""", RegexOption.IGNORE_CASE)

    private val vidRe = Regex("""\.(mp4|mkv|webm|avi|mov|m4v|wmv|flv|ts|m2ts|mpg|mpeg|vob|ogv|3gp)$""", RegexOption.IGNORE_CASE)
    fun looksLikeFile(s: String) = extRe.containsMatchIn(s.trim())
    /** Pure numbers (MediaStore ids like 8387) are never a usable title or file name. */
    fun usable(s: String?) = s != null && s.isNotBlank() && !s.trim().all { it.isDigit() } && s.trim() != "<unknown>"
    @Volatile var diag = ""
    fun stripExt(s: String) = s.replace(extRe, "")
    fun safeTake(s: String, n: Int): String { if (s.length <= n) return s; val e = if (Character.isHighSurrogate(s[n - 1])) n - 1 else n; return s.substring(0, e) }

    fun resolve(ctx: Context, md: MediaMetadata, sessionTitle0: String, dur: Long, notifTitle: String, notifText: String, onReady: () -> Unit): TrackInfo {
        val sessionTitle = listOf(sessionTitle0, md.getString(MediaMetadata.METADATA_KEY_DISPLAY_TITLE), notifTitle).firstOrNull { usable(it) } ?: sessionTitle0
        val p = Prefs(ctx)
        val sArtist = listOf(md.getString(MediaMetadata.METADATA_KEY_ARTIST), md.getString(MediaMetadata.METADATA_KEY_ALBUM_ARTIST), md.getString(MediaMetadata.METADATA_KEY_DISPLAY_SUBTITLE), notifText).firstOrNull { usable(it) } ?: ""
        val sAlbum = md.getString(MediaMetadata.METADATA_KEY_ALBUM) ?: ""
        val sYear = md.getLong(MediaMetadata.METADATA_KEY_YEAR).let { if (it > 0) it.toString() else "" }
        val display = md.getString(MediaMetadata.METADATA_KEY_DISPLAY_TITLE) ?: ""
        val titleIsFile = looksLikeFile(sessionTitle) || (display.isNotEmpty() && display == sessionTitle && looksLikeFile(display))

        val key = (md.getString(MediaMetadata.METADATA_KEY_MEDIA_URI) ?: md.getString(MediaMetadata.METADATA_KEY_MEDIA_ID) ?: "") + "|$sessionTitle|$dur"
        val tags = cache[key]
        if (tags == null && busy.add(key)) {
            ex.execute {
                val t = try { read(ctx, md, sessionTitle, dur) } catch (_: Throwable) { null }
                cache[key] = t ?: FileTags("", "", "", "", if (titleIsFile) sessionTitle else "")
                busy.remove(key)
                if (cache.size > 200) cache.clear()
                onReady()
            }
        }
        val f0 = tags ?: FileTags("", "", "", "", if (titleIsFile) sessionTitle else "")
        val f = f0.copy(title = f0.title.takeIf { usable(it) } ?: "", artist = f0.artist.takeIf { usable(it) } ?: "",
            filename = f0.filename.takeIf { usable(stripExt(it)) } ?: if (titleIsFile) sessionTitle else "")

        val tagTitle = f.title.ifEmpty { if (titleIsFile) "" else sessionTitle }
        val artist = f.artist.ifEmpty { sArtist }.let { if (it == "<unknown>" || it.equals("Unknown artist", true)) "" else it }
        val album = f.album.ifEmpty { sAlbum }
        val year = f.year.ifEmpty { sYear }
        var filename = f.filename.ifEmpty { if (titleIsFile) sessionTitle else "" }
        if (p.stripExt) filename = stripExt(filename)

        val title = when (p.source) {
            1 -> filename.ifEmpty { tagTitle }
            2 -> tagTitle.ifEmpty { if (p.stripExt) stripExt(sessionTitle) else sessionTitle }
            else -> tagTitle.ifEmpty { filename.ifEmpty { sessionTitle } }
        }
        val vars = mapOf("title" to title, "titel" to title, "artist" to artist, "artiest" to artist,
            "album" to album, "year" to year, "jaar" to year, "filename" to filename, "bestandsnaam" to filename, "type" to (if (vidRe.containsMatchIn(sessionTitle) || f.video) "video" else "music"), "soort" to (if (vidRe.containsMatchIn(sessionTitle) || f.video) "video" else "muziek"))
        fun fill(t: String) = Regex("""\{(\w+)\}""").replace(t) { m -> vars[m.groupValues[1].lowercase()] ?: m.value }
            .replace(Regex("""\s+"""), " ").trim().trim('-', '–', '•', '|', ' ')
        val details = fill(p.detailsTpl).ifEmpty { title }
        val usesArtist = Regex("""\{(artist|artiest)\}""").containsMatchIn(p.stateTpl)
        val state = if (usesArtist && artist.isEmpty()) p.noArtist.trim() else fill(p.stateTpl)
        diag = buildString {
            for (k in md.keySet().sorted()) { val v = try { md.getString(k) ?: md.getLong(k).toString() } catch (_: Exception) { "?" }; append("$k = $v\n") }
            append("notif.title = $notifTitle\nnotif.text = $notifText\n")
            append("bestand: ${f0.filename} | tags: ${f0.title} / ${f0.artist} / ${f0.album} / ${f0.year}\n")
            append("uri: ${lastUri[key] ?: "-"}\n")
        }
        val isVideo = f0.video || vidRe.containsMatchIn(f0.filename) || vidRe.containsMatchIn(sessionTitle) || vidRe.containsMatchIn(sessionTitle0)
        return TrackInfo(isVideo, title, artist, album, year, filename, safeTake(details, 120), safeTake(state, 120))
    }

    private fun read(ctx: Context, md: MediaMetadata, title: String, dur: Long): FileTags? {
        val uri = findUri(ctx, md, title, dur) ?: return null
        lastUri[(md.getString(MediaMetadata.METADATA_KEY_MEDIA_URI) ?: md.getString(MediaMetadata.METADATA_KEY_MEDIA_ID) ?: "") + "|$title|$dur"] = uri.toString()
        var name = ""
        if (uri.scheme == "content") {
            try { ctx.contentResolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) name = it.getString(0) ?: "" } } catch (_: Exception) {}
        }
        if (name.isEmpty()) name = Uri.decode(uri.lastPathSegment ?: "").substringAfterLast('/')
        if (!usable(stripExt(name))) name = ""
        val r = MediaMetadataRetriever()
        return try {
            if (uri.scheme == "file" || uri.scheme == null) r.setDataSource(uri.path) else r.setDataSource(ctx, uri)
            fun g(k: Int) = r.extractMetadata(k)?.trim() ?: ""
            val year = g(MediaMetadataRetriever.METADATA_KEY_YEAR).ifEmpty { g(MediaMetadataRetriever.METADATA_KEY_DATE).take(4) }
                .let { if (it == "0") "" else it }
            FileTags(g(MediaMetadataRetriever.METADATA_KEY_TITLE),
                g(MediaMetadataRetriever.METADATA_KEY_ARTIST).ifEmpty { g(MediaMetadataRetriever.METADATA_KEY_ALBUMARTIST) },
                g(MediaMetadataRetriever.METADATA_KEY_ALBUM), year, name, g(MediaMetadataRetriever.METADATA_KEY_HAS_VIDEO) == "yes")
        } catch (_: Exception) { FileTags("", "", "", "", name) } finally { try { r.release() } catch (_: Exception) {} }
    }

    private fun findUri(ctx: Context, md: MediaMetadata, title: String, dur: Long): Uri? {
        for (k in listOf(MediaMetadata.METADATA_KEY_MEDIA_URI, MediaMetadata.METADATA_KEY_MEDIA_ID)) {
            val v = md.getString(k) ?: continue
            if (v.startsWith("content://") || v.startsWith("file://")) return Uri.parse(v)
            if (v.startsWith("/")) return Uri.fromFile(java.io.File(v))
            if (v.isNotEmpty() && v.all { it.isDigit() }) return ContentUris.withAppendedId(MediaStore.Files.getContentUri("external"), v.toLong())
        }
        // MediaStore lookup: by file name, then by title, closest duration wins
        for (coll in listOf(MediaStore.Audio.Media.EXTERNAL_CONTENT_URI, MediaStore.Video.Media.EXTERNAL_CONTENT_URI)) {
        val proj = arrayOf(MediaStore.MediaColumns._ID, MediaStore.MediaColumns.DURATION)
        val tries = listOf("${MediaStore.MediaColumns.DISPLAY_NAME}=?" to title,
                           "${MediaStore.MediaColumns.TITLE}=?" to title,
                           "${MediaStore.MediaColumns.DISPLAY_NAME} LIKE ?" to "$title.%")
        for ((sel, arg) in tries) {
            try {
                ctx.contentResolver.query(coll, proj, sel, arrayOf(arg), null)?.use { c ->
                    var best: Long = -1; var bestDiff = Long.MAX_VALUE
                    while (c.moveToNext()) {
                        val d = if (dur > 0) kotlin.math.abs(c.getLong(1) - dur) else 0
                        if (d < bestDiff) { bestDiff = d; best = c.getLong(0) }
                    }
                    if (best >= 0 && (dur <= 0 || bestDiff < 3000)) return ContentUris.withAppendedId(coll, best)
                }
            } catch (_: Exception) { }
        }
        }
        return null
    }
}
