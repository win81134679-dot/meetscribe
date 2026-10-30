package io.github.win81134679.meetscribe.service

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import io.github.win81134679.meetscribe.R
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.ui.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/**
 * 有專案在處理時常駐的前景服務：讓系統不要在關螢幕、切到別的 App 時把轉錄中斷，並在通知列顯示進度。
 * 沒有工作了就自己結束。中途還是被收掉的話，專案下次開 App 會標成「已取消」，可以續跑（流程每步都有存檔）。
 */
class JobService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var wakeLock: PowerManager.WakeLock? = null
    private var watching = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        ensureChannel()
        val type = if (Build.VERSION.SDK_INT >= 29) ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, build("準備中…", null), type)
        if (wakeLock == null) {
            wakeLock = (getSystemService(POWER_SERVICE) as PowerManager)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "MeetScribe:transcribe")
                .apply { acquire(6 * 60 * 60 * 1000L) }
        }
        if (!watching) {
            watching = true
            scope.launch {
                combine(Repo.busy, Repo.progress, Repo.projects) { busy, progress, projects -> Triple(busy, progress, projects) }
                    .collect { (busy, progress, projects) ->
                        if (busy.isEmpty()) {
                            stop()
                            return@collect
                        }
                        val id = busy.first()
                        val title = projects.firstOrNull { it.id == id }?.title ?: "會議"
                        val p = progress[id]
                        val text = buildString {
                            append(title)
                            if (p != null) append("：${p.stageLabel}　${p.message}")
                            if (busy.size > 1) append("（另有 ${busy.size - 1} 場排隊）")
                        }
                        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, build(text, p?.fraction))
                    }
            }
        }
        return START_NOT_STICKY
    }

    /** Android 15 起 dataSync 前景服務一天最多 6 小時；時間到先停，專案可續跑。 */
    override fun onTimeout(startId: Int, fgsType: Int) {
        stop()
    }

    private fun stop() {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        scope.cancel()
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        super.onDestroy()
    }

    private fun ensureChannel() {
        val nm = getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, getString(R.string.channel_jobs), NotificationManager.IMPORTANCE_LOW),
            )
        }
    }

    private fun build(text: String, fraction: Float?) = NotificationCompat.Builder(this, CHANNEL)
        .setSmallIcon(R.drawable.ic_notification)
        .setContentTitle("正在產生逐字稿")
        .setContentText(text)
        .setStyle(NotificationCompat.BigTextStyle().bigText(text))
        .setOngoing(true)
        .setOnlyAlertOnce(true)
        .setProgress(100, ((fraction ?: 0f) * 100).toInt(), fraction == null)
        .setContentIntent(
            PendingIntent.getActivity(
                this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_IMMUTABLE,
            ),
        )
        .build()

    companion object {
        private const val CHANNEL = "jobs"
        private const val NOTIFICATION_ID = 1
    }
}
