package io.github.win81134679.meetscribe.ui

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/** 匯出檔、錄音的分享與開啟（經 FileProvider，只開放匯出資料夾與分享用的暫存區）。 */
object Files {
    private fun uri(ctx: Context, f: File): Uri = FileProvider.getUriForFile(ctx, "${ctx.packageName}.files", f)

    fun mime(f: File): String = when (f.extension.lowercase()) {
        "md" -> "text/markdown"
        "srt" -> "application/x-subrip"
        "vtt" -> "text/vtt"
        "txt" -> "text/plain"
        "csv" -> "text/csv"
        "json" -> "application/json"
        "m4a", "mp4a" -> "audio/mp4"
        "aac" -> "audio/aac"
        "mp3" -> "audio/mpeg"
        "wav" -> "audio/wav"
        "ogg", "opus" -> "audio/ogg"
        "amr" -> "audio/amr"
        "flac" -> "audio/flac"
        else -> MimeTypeMap.getSingleton().getMimeTypeFromExtension(f.extension.lowercase()) ?: "application/octet-stream"
    }

    fun share(ctx: Context, files: List<File>) {
        if (files.isEmpty()) return
        val uris = ArrayList(files.map { uri(ctx, it) })
        val intent = if (uris.size == 1) {
            Intent(Intent.ACTION_SEND).setType(mime(files[0])).putExtra(Intent.EXTRA_STREAM, uris[0])
        } else {
            Intent(Intent.ACTION_SEND_MULTIPLE).setType("*/*").putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
        }
        intent.clipData = ClipData.newRawUri(null, uris[0]).apply { uris.drop(1).forEach { addItem(ClipData.Item(it)) } }
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        ctx.startActivity(Intent.createChooser(intent, "分享").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    /**
     * 分享錄音（2026-10-31 使用者：「分享錄音檔：直接傳到 LINE、雲端硬碟」）。App 裡的錄音檔名是 playback.m4a 這類，
     * 先以標題為檔名複製到暫存區（cache/share）再分享，對方收到的才是「週會.m4a」。一小時以前的暫存檔順手清掉。
     */
    suspend fun shareAudio(ctx: Context, src: File, title: String) {
        val f = withContext(Dispatchers.IO) {
            val dir = File(ctx.cacheDir, "share").apply { mkdirs() }
            dir.listFiles()?.filter { System.currentTimeMillis() - it.lastModified() > 3_600_000 }?.forEach { it.delete() }
            val name = title.replace(Regex("""[\/:*?"<>|\u0000-\u001f]"""), "_").trim().take(100).ifEmpty { "錄音" }
            File(dir, "$name.${src.extension.ifEmpty { "m4a" }}").also { src.copyTo(it, overwrite = true) }
        }
        share(ctx, listOf(f))
    }

    /** 用其他 App 開啟；找不到能開的 App 回 false。 */
    fun open(ctx: Context, f: File): Boolean = try {
        ctx.startActivity(
            Intent(Intent.ACTION_VIEW).setDataAndType(uri(ctx, f), mime(f))
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        true
    } catch (_: ActivityNotFoundException) {
        false
    }

    fun openUrl(ctx: Context, url: String) {
        runCatching { ctx.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
    }

    fun shareText(ctx: Context, text: String) {
        val i = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
        ctx.startActivity(Intent.createChooser(i, "分享").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }
}

/** 外部網址集中在這裡（同桌面 theme::links）。 */
object Links {
    const val OPENROUTER_CREDITS = "https://openrouter.ai/settings/credits"
    const val OPENROUTER_KEYS = "https://openrouter.ai/settings/keys"
    const val GOOGLE_AI_STUDIO_KEYS = "https://aistudio.google.com/apikey"
}
