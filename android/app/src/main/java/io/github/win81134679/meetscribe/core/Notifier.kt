package io.github.win81134679.meetscribe.core

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import io.github.win81134679.meetscribe.R
import io.github.win81134679.meetscribe.ui.MainActivity

/**
 * 一次性的通知：會議處理完成（點了直接打開那場會議）、新版已下載、已更新到新版。
 * 進度通知在 JobService，錄音中的通知在 RecorderService。
 */
object Notifier {
    const val EXTRA_OPEN_PROJECT = "open_project"
    const val EXTRA_OPEN_SETTINGS = "open_settings"
    private const val CHANNEL_DONE = "done"
    private const val CHANNEL_UPDATE = "update"
    private const val ID_UPDATE = 9001

    private fun ensure(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL_DONE) == null) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL_DONE, "處理完成", NotificationManager.IMPORTANCE_DEFAULT))
        }
        if (nm.getNotificationChannel(CHANNEL_UPDATE) == null) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL_UPDATE, "App 更新", NotificationManager.IMPORTANCE_LOW))
        }
    }

    private fun allowed(ctx: Context) =
        ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED ||
            android.os.Build.VERSION.SDK_INT < 33

    private fun open(ctx: Context, requestCode: Int, extra: Pair<String, String>?): PendingIntent {
        val i = Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
        extra?.let { i.putExtra(it.first, it.second) }
        return PendingIntent.getActivity(ctx, requestCode, i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    fun projectDone(ctx: Context, id: String, title: String) {
        if (!allowed(ctx)) return
        ensure(ctx)
        val n = NotificationCompat.Builder(ctx, CHANNEL_DONE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("「$title」處理完成")
            .setContentText("逐字稿與摘要都好了，點一下打開")
            .setAutoCancel(true)
            .setContentIntent(open(ctx, id.hashCode(), EXTRA_OPEN_PROJECT to id))
            .build()
        runCatching { NotificationManagerCompat.from(ctx).notify(id.hashCode(), n) }
    }

    fun updateReady(ctx: Context, version: String) {
        if (!allowed(ctx)) return
        ensure(ctx)
        val n = NotificationCompat.Builder(ctx, CHANNEL_UPDATE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("語音轉錄 $version 已備妥")
            .setContentText("點一下打開，按「更新」就會安裝")
            .setAutoCancel(true)
            .setContentIntent(open(ctx, ID_UPDATE, EXTRA_OPEN_SETTINGS to "1"))
            .build()
        runCatching { NotificationManagerCompat.from(ctx).notify(ID_UPDATE, n) }
    }

    fun updated(ctx: Context, version: String) {
        if (!allowed(ctx)) return
        ensure(ctx)
        val n = NotificationCompat.Builder(ctx, CHANNEL_UPDATE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("已更新到語音轉錄 $version")
            .setContentText("點一下打開")
            .setAutoCancel(true)
            .setContentIntent(open(ctx, ID_UPDATE, null))
            .build()
        runCatching { NotificationManagerCompat.from(ctx).notify(ID_UPDATE, n) }
    }
}
