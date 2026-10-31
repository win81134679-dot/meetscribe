package io.github.win81134679.meetscribe.core

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.os.Build
import android.os.Environment
import android.provider.DocumentsContract
import androidx.core.content.ContextCompat
import java.io.File

/**
 * 匯出檔放在公開的「文件/語音轉錄/<會議>」（2026-10-31 使用者：「手機版支援開啟該會議資料夾功能」）：
 * 系統的「檔案」App、接電腦用 USB 都看得到。1.0.0 放在 App 的外部檔案區（Android/data/…），Android 11 起別的 App 看不到。
 *
 * Android 11+ 不需要權限就能在 Documents 建自己的檔；10 靠 manifest 的 requestLegacyExternalStorage；
 * 8–9 要 WRITE_EXTERNAL_STORAGE（manifest 只宣告到 API 28）。寫不進去就退回 App 的外部檔案區。
 */
object Storage {
    const val FOLDER_NAME = "語音轉錄"

    /** 公開的匯出資料夾；這台不能寫就回 null。 */
    fun publicExportsDir(ctx: Context): File? {
        if (Build.VERSION.SDK_INT <= 28 &&
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED
        ) {
            return null
        }
        return runCatching {
            @Suppress("DEPRECATION")
            val docs = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS)
            val dir = File(docs, FOLDER_NAME)
            dir.mkdirs()
            val probe = File(dir, ".write-test-${System.nanoTime()}")
            probe.writeText("x")
            probe.delete()
            dir.takeIf { it.isDirectory }
        }.getOrNull()
    }

    fun exportsDir(ctx: Context): File =
        publicExportsDir(ctx) ?: ctx.getExternalFilesDir("exports") ?: File(ctx.filesDir, "exports")

    /** 刪掉的檔也通知媒體庫，接電腦時才不會看到已經不在的檔。 */
    fun forget(ctx: Context, paths: List<String>) {
        if (paths.isNotEmpty()) MediaScannerConnection.scanFile(ctx, paths.toTypedArray(), null, null)
    }

    /** 讓剛寫好的檔馬上出現在接電腦（MTP）與其他 App 的清單裡。 */
    fun scan(ctx: Context, files: List<File>) {
        val paths = files.filter { it.isFile }.map { it.absolutePath }
        if (paths.isNotEmpty()) MediaScannerConnection.scanFile(ctx, paths.toTypedArray(), null, null)
    }

    /**
     * 用系統「檔案」App 打開資料夾。只有在共用儲存空間（/storage/emulated/0/…）的資料夾開得起來；
     * 開不起來（App 私有區、手機沒有能開資料夾的 App）回 false，呼叫端改顯示 App 內的檔案清單。
     */
    fun openFolder(ctx: Context, dir: File): Boolean {
        @Suppress("DEPRECATION")
        val root = Environment.getExternalStorageDirectory().absolutePath.trimEnd('/')
        val path = dir.absolutePath
        if (!path.startsWith("$root/") || path.contains("/Android/data/")) return false
        val docId = "primary:" + path.removePrefix("$root/")
        val uri = DocumentsContract.buildDocumentUri("com.android.externalstorage.documents", docId)
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, DocumentsContract.Document.MIME_TYPE_DIR)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_GRANT_READ_URI_PERMISSION)
        return try {
            ctx.startActivity(intent)
            true
        } catch (_: ActivityNotFoundException) {
            false
        } catch (_: SecurityException) {
            false
        }
    }

    /** 給使用者看的位置：「文件/語音轉錄/週會」。 */
    fun displayPath(dir: File): String {
        @Suppress("DEPRECATION")
        val root = Environment.getExternalStorageDirectory().absolutePath.trimEnd('/')
        val rel = dir.absolutePath.removePrefix("$root/")
        return if (rel.startsWith("Documents/")) "文件/" + rel.removePrefix("Documents/") else rel
    }
}
