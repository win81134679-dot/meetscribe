package io.github.win81134679.meetscribe.service

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.MediaRecorder
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import io.github.win81134679.meetscribe.R
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.ui.MainActivity
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID

/** 錄音狀態（畫面讀這裡；錄音本身在 [RecorderService]，關螢幕、切到別的 App 都繼續錄）。 */
object Recording {
    enum class Phase { Idle, Recording, Paused }

    data class State(
        val phase: Phase = Phase.Idle,
        /** 已錄的毫秒數（暫停前的累計）。 */
        val accumulatedMs: Long = 0,
        /** 這一段開始的 elapsedRealtime；暫停時是 0。 */
        val since: Long = 0,
    ) {
        fun elapsedMs(now: Long = SystemClock.elapsedRealtime()) = accumulatedMs + if (since > 0) now - since else 0
    }

    val state = MutableStateFlow(State())
    /** 錄完（按「停止」）的檔案：新增轉錄頁收到就當成選好的錄音。 */
    val finished = MutableSharedFlow<Pair<File, String>>(replay = 1, extraBufferCapacity = 1)

    internal var recorder: MediaRecorder? = null
    internal var file: File? = null
    internal var name: String = ""

    /** 目前音量 0..1（畫面上的音量條）。 */
    fun level(): Float = runCatching { (recorder?.maxAmplitude ?: 0) / 32767f }.getOrDefault(0f).coerceIn(0f, 1f)

    fun start(ctx: Context) = send(ctx, RecorderService.ACTION_START)
    fun pause(ctx: Context) = send(ctx, RecorderService.ACTION_PAUSE)
    fun resume(ctx: Context) = send(ctx, RecorderService.ACTION_RESUME)
    fun stop(ctx: Context) = send(ctx, RecorderService.ACTION_STOP)
    fun discard(ctx: Context) = send(ctx, RecorderService.ACTION_DISCARD)

    private fun send(ctx: Context, action: String) {
        val i = Intent(ctx, RecorderService::class.java).setAction(action)
        if (action == RecorderService.ACTION_START) ContextCompat.startForegroundService(ctx, i) else ctx.startService(i)
    }
}

/**
 * App 內錄音（2026-10-31：「更多貼心設計等你設計」——手機本身就是錄音機，錄完直接轉錄）。
 * AAC／M4A 單聲道 44.1 kHz 64 kbps（一小時約 29 MB），檔案直接寫在匯入資料夾，錄完不必再複製。
 * 前景服務（microphone）讓關螢幕也繼續錄；通知列顯示「錄音中」並可點回 App。
 */
class RecorderService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> start()
            ACTION_PAUSE -> pause()
            ACTION_RESUME -> resume()
            ACTION_STOP -> finish(keep = true)
            ACTION_DISCARD -> finish(keep = false)
        }
        return START_NOT_STICKY
    }

    private fun start() {
        if (Recording.recorder != null) return
        ensureChannel()
        val type = if (Build.VERSION.SDK_INT >= 30) ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification("錄音中"), type)
        val stamp = SimpleDateFormat("yyyy-MM-dd HHmm", Locale.TAIWAN).format(Date())
        val name = "會議錄音 $stamp"
        val dir = File(Repo.importsDir, UUID.randomUUID().toString()).apply { mkdirs() }
        val file = File(dir, "$name.m4a")
        val rec = (if (Build.VERSION.SDK_INT >= 31) MediaRecorder(this) else @Suppress("DEPRECATION") MediaRecorder())
        try {
            rec.setAudioSource(MediaRecorder.AudioSource.MIC)
            rec.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
            rec.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
            rec.setAudioChannels(1)
            rec.setAudioSamplingRate(44100)
            rec.setAudioEncodingBitRate(64000)
            rec.setOutputFile(file.absolutePath)
            rec.prepare()
            rec.start()
        } catch (e: Exception) {
            rec.release()
            dir.deleteRecursively()
            Repo.toasts.tryEmit(true to "無法開始錄音：${e.message ?: e.javaClass.simpleName}")
            stopSelfNow()
            return
        }
        Recording.recorder = rec
        Recording.file = file
        Recording.name = name
        Recording.state.value = Recording.State(Recording.Phase.Recording, 0, SystemClock.elapsedRealtime())
    }

    private fun pause() {
        val rec = Recording.recorder ?: return
        val s = Recording.state.value
        if (s.phase != Recording.Phase.Recording) return
        runCatching { rec.pause() }
        Recording.state.value = Recording.State(Recording.Phase.Paused, s.elapsedMs(), 0)
        update("已暫停")
    }

    private fun resume() {
        val rec = Recording.recorder ?: return
        val s = Recording.state.value
        if (s.phase != Recording.Phase.Paused) return
        runCatching { rec.resume() }
        Recording.state.value = Recording.State(Recording.Phase.Recording, s.accumulatedMs, SystemClock.elapsedRealtime())
        update("錄音中")
    }

    private fun finish(keep: Boolean) {
        val rec = Recording.recorder
        val file = Recording.file
        Recording.recorder = null
        Recording.file = null
        val ok = rec != null && runCatching { rec.stop() }.isSuccess
        rec?.release()
        Recording.state.value = Recording.State()
        if (file != null) {
            if (keep && ok && file.length() > 0) {
                Recording.finished.tryEmit(file to Recording.name)
            } else {
                file.parentFile?.deleteRecursively()
                if (keep) Repo.toasts.tryEmit(true to "錄音太短或沒有錄到聲音")
            }
        }
        stopSelfNow()
    }

    private fun stopSelfNow() {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        // 被系統收掉：把錄到的部分存起來（MediaRecorder.stop 會寫好檔尾）
        if (Recording.recorder != null) finish(keep = true)
        super.onDestroy()
    }

    private fun ensureChannel() {
        val nm = getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, "錄音", NotificationManager.IMPORTANCE_LOW))
        }
    }

    private fun update(text: String) {
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(text))
    }

    private fun notification(text: String) = NotificationCompat.Builder(this, CHANNEL)
        .setSmallIcon(R.drawable.ic_notification)
        .setContentTitle("語音轉錄：$text")
        .setContentText("點一下回到 App 停止或暫停")
        .setOngoing(true)
        .setOnlyAlertOnce(true)
        .setUsesChronometer(text == "錄音中")
        .setWhen(System.currentTimeMillis() - Recording.state.value.elapsedMs())
        .setContentIntent(
            PendingIntent.getActivity(
                this, 2, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_IMMUTABLE,
            ),
        )
        .build()

    companion object {
        const val ACTION_START = "start"
        const val ACTION_PAUSE = "pause"
        const val ACTION_RESUME = "resume"
        const val ACTION_STOP = "stop"
        const val ACTION_DISCARD = "discard"
        private const val CHANNEL = "recording"
        private const val NOTIFICATION_ID = 2
    }
}
