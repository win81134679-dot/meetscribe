package io.github.win81134679.meetscribe.ui

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File

/** 匯出檔的分享與開啟（經 FileProvider，只開放匯出資料夾）。 */
object Files {
    private fun uri(ctx: Context, f: File): Uri = FileProvider.getUriForFile(ctx, "${ctx.packageName}.files", f)

    fun mime(f: File): String = when (f.extension.lowercase()) {
        "md" -> "text/markdown"
        "srt" -> "application/x-subrip"
        "vtt" -> "text/vtt"
        "txt" -> "text/plain"
        "csv" -> "text/csv"
        "json" -> "application/json"
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
