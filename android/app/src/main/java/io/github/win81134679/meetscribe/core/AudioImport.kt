package io.github.win81134679.meetscribe.core

import android.content.Context
import android.media.MediaCodec
import android.media.MediaExtractor
import android.media.MediaFormat
import android.net.Uri
import android.provider.OpenableColumns
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.UUID
import kotlin.coroutines.coroutineContext

/** 匯入好的錄音：App 私有資料夾裡的副本（Rust 讀不到 content:// 網址）。 */
data class ImportedAudio(val file: File, val displayName: String, val duration: Double)

/**
 * 把使用者選的錄音複製進 App，再請 Rust 讀長度。
 * Rust（symphonia）讀不了的格式（Opus、AMR、3GP、WebM…）改用系統解碼器轉成 16 kHz 單聲道 WAV。
 */
object AudioImport {
    suspend fun import(ctx: Context, uri: Uri, onStatus: (String) -> Unit): ImportedAudio = withContext(Dispatchers.IO) {
        val name = displayName(ctx, uri) ?: "錄音"
        // imports/<uuid>/<原檔名>：保留檔名（匯出檔的「原始檔案」顯示它）；核心啟動時會清掉沒有專案在用的資料夾
        val dir = File(Repo.importsDir, UUID.randomUUID().toString()).apply { mkdirs() }
        val copy = File(dir, safeName(name))
        try {
            onStatus("複製檔案…")
            val input = try {
                ctx.contentResolver.openInputStream(uri)
            } catch (_: SecurityException) {
                throw BridgeError("沒有權限讀取這個檔案，請改用「選擇錄音檔」")
            } ?: throw BridgeError("無法開啟這個檔案")
            input.use { i -> copy.outputStream().use { i.copyTo(it, 1 shl 16) } }

            onStatus("讀取音訊資訊…")
            probe(copy)?.let { return@withContext ImportedAudio(copy, name, it) }

            onStatus("轉換音訊格式…")
            val wav = File(dir, "${copy.nameWithoutExtension}.wav")
            try {
                decodeToWav(copy, wav)
            } catch (e: Exception) {
                throw BridgeError("無法讀取這個檔案的音軌：${e.message ?: e.javaClass.simpleName}")
            }
            copy.delete()
            val d = probe(wav) ?: throw BridgeError("轉換後仍無法讀取音訊")
            ImportedAudio(wav, name, d)
        } catch (e: Exception) {
            dir.deleteRecursively()
            throw e
        }
    }

    /** App 內錄好的檔（已經在 imports/<uuid>/ 裡）：只讀長度，讀不到就交給系統解碼器轉 WAV。 */
    suspend fun fromFile(file: File, displayName: String): ImportedAudio = withContext(Dispatchers.IO) {
        probe(file)?.let { return@withContext ImportedAudio(file, displayName, it) }
        val wav = File(file.parentFile, "${file.nameWithoutExtension}.wav")
        try {
            decodeToWav(file, wav)
        } catch (e: Exception) {
            throw BridgeError("讀不到錄好的音檔：${e.message ?: e.javaClass.simpleName}")
        }
        file.delete()
        ImportedAudio(wav, displayName, probe(wav) ?: throw BridgeError("讀不到錄好的音檔"))
    }

    /** 丟掉一個還沒開始轉錄的匯入（整個 imports/<uuid> 資料夾）。 */
    fun discard(a: ImportedAudio) {
        val dir = a.file.parentFile ?: return
        if (dir.parentFile == Repo.importsDir) dir.deleteRecursively()
    }

    private fun safeName(name: String): String =
        name.replace(Regex("""[\\/:*?"<>|\u0000-\u001f]"""), "_").trim().take(120).ifEmpty { "錄音" }

    private fun probe(f: File): Double? =
        runCatching { (Repo.callSync("probe", mapOf("path" to f.absolutePath)) as Number).toDouble() }.getOrNull()

    fun displayName(ctx: Context, uri: Uri): String? =
        runCatching {
            ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) c.getString(0) else null
            }
        }.getOrNull() ?: uri.lastPathSegment?.substringAfterLast('/')

    /** MediaExtractor＋MediaCodec 解碼成 16-bit PCM，混成單聲道、線性內插到 16 kHz，寫成 WAV。 */
    private suspend fun decodeToWav(input: File, output: File) {
        val extractor = MediaExtractor()
        extractor.setDataSource(input.absolutePath)
        val track = (0 until extractor.trackCount).firstOrNull {
            extractor.getTrackFormat(it).getString(MediaFormat.KEY_MIME)?.startsWith("audio/") == true
        } ?: error("沒有音軌")
        extractor.selectTrack(track)
        val format = extractor.getTrackFormat(track)
        val codec = MediaCodec.createDecoderByType(format.getString(MediaFormat.KEY_MIME)!!)
        codec.configure(format, null, null, 0)
        codec.start()

        val out = RandomAccessFile(output, "rw")
        out.setLength(0)
        out.write(ByteArray(44)) // 標頭最後再補
        var srcRate = format.getInteger(MediaFormat.KEY_SAMPLE_RATE)
        var channels = format.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
        var pos = 0.0 // 下一個輸出點在來源的位置
        var srcIndex = 0L
        var prev = 0f
        var written = 0L
        val info = MediaCodec.BufferInfo()
        var inputDone = false
        val pcmOut = ByteBuffer.allocate(1 shl 16).order(ByteOrder.LITTLE_ENDIAN)
        try {
            while (true) {
                coroutineContext.ensureActive()
                if (!inputDone) {
                    val i = codec.dequeueInputBuffer(10_000)
                    if (i >= 0) {
                        val buf = codec.getInputBuffer(i)!!
                        val n = extractor.readSampleData(buf, 0)
                        if (n < 0) {
                            codec.queueInputBuffer(i, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                            inputDone = true
                        } else {
                            codec.queueInputBuffer(i, 0, n, extractor.sampleTime, 0)
                            extractor.advance()
                        }
                    }
                }
                val o = codec.dequeueOutputBuffer(info, 10_000)
                when {
                    o == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        val f = codec.outputFormat
                        srcRate = f.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                        channels = f.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                    }
                    o >= 0 -> {
                        val buf = codec.getOutputBuffer(o)!!.order(ByteOrder.LITTLE_ENDIAN)
                        buf.position(info.offset)
                        buf.limit(info.offset + info.size)
                        val shorts = buf.asShortBuffer()
                        val frames = shorts.remaining() / channels.coerceAtLeast(1)
                        val step = srcRate / 16000.0
                        for (fi in 0 until frames) {
                            var sum = 0f
                            for (c in 0 until channels) sum += shorts.get(fi * channels + c)
                            val cur = sum / channels
                            // 輸出點落在 (srcIndex-1, srcIndex] 之間就內插
                            while (pos <= srcIndex) {
                                val frac = (pos - (srcIndex - 1)).toFloat().coerceIn(0f, 1f)
                                val v = (prev + (cur - prev) * frac).toInt().coerceIn(-32768, 32767)
                                if (pcmOut.remaining() < 2) {
                                    out.write(pcmOut.array(), 0, pcmOut.position())
                                    pcmOut.clear()
                                }
                                pcmOut.putShort(v.toShort())
                                written += 2
                                pos += step
                            }
                            prev = cur
                            srcIndex++
                        }
                        codec.releaseOutputBuffer(o, false)
                        if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
                    }
                }
            }
            out.write(pcmOut.array(), 0, pcmOut.position())
            out.seek(0)
            out.write(wavHeader(written, 16000))
        } finally {
            out.close()
            runCatching { codec.stop() }
            codec.release()
            extractor.release()
        }
    }

    private fun wavHeader(dataBytes: Long, rate: Int): ByteArray {
        val b = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN)
        b.put("RIFF".toByteArray()).putInt((36 + dataBytes).toInt()).put("WAVE".toByteArray())
        b.put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(1).putInt(rate).putInt(rate * 2).putShort(2).putShort(16)
        b.put("data".toByteArray()).putInt(dataBytes.toInt())
        return b.array()
    }
}
