package io.github.win81134679.meetscribe.core

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.IntentCompat
import java.io.File

/**
 * 手機版的「安裝」那一半（檢查、下載、驗簽章在 Rust：meetscribe-core/src/update.rs）。
 *
 * Android 不允許商店以外的 App 完全不經使用者同意就裝東西，做法是：
 * - 新版在背景自動下載、驗證好，畫面上出現「新版已備妥」，按一下就裝。
 * - 第一次要使用者在系統設定允許本 App「安裝不明應用程式」（[canInstall]）。
 * - Android 12 起用 USER_ACTION_NOT_REQUIRED：App 更新自己時系統可以不再跳確認視窗（版本夠新、有
 *   UPDATE_PACKAGES_WITHOUT_USER_ACTION 權限）；系統仍要確認時，[InstallReceiver] 把確認畫面叫出來。
 * - 裝好後 App 會被系統關掉；[UpdatedReceiver] 收到 MY_PACKAGE_REPLACED 發一則「已更新」通知。
 * 新舊 APK 必須是同一把簽章金鑰（android/keystore/，見 CLAUDE.md），否則系統拒裝。
 */
object Updater {
    fun canInstall(ctx: Context): Boolean =
        Build.VERSION.SDK_INT < 26 || ctx.packageManager.canRequestPackageInstalls()

    /** 系統設定裡「允許這個 App 安裝不明應用程式」那一頁。 */
    fun openInstallPermission(ctx: Context) {
        val i = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${ctx.packageName}"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { ctx.startActivity(i) }
    }

    /** 把下載好的 APK 交給 PackageInstaller。結果經 [InstallReceiver] 回來。 */
    fun install(ctx: Context, apk: File) {
        val installer = ctx.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(ctx.packageName)
            if (Build.VERSION.SDK_INT >= 31) setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            if (Build.VERSION.SDK_INT >= 33) setPackageSource(PackageInstaller.PACKAGE_SOURCE_OTHER)
        }
        val id = installer.createSession(params)
        installer.openSession(id).use { session ->
            session.openWrite("base.apk", 0, apk.length()).use { out ->
                apk.inputStream().use { it.copyTo(out, 1 shl 16) }
                session.fsync(out)
            }
            val callback = PendingIntent.getBroadcast(
                ctx, id, Intent(ctx, InstallReceiver::class.java).setPackage(ctx.packageName),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
            )
            session.commit(callback.intentSender)
        }
    }
}

/** PackageInstaller 的結果：要使用者確認就叫出確認畫面；失敗就提示原因。 */
class InstallReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        when (val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                IntentCompat.getParcelableExtra(intent, Intent.EXTRA_INTENT, Intent::class.java)?.let {
                    runCatching { ctx.startActivity(it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
                }
            }
            PackageInstaller.STATUS_SUCCESS -> {}
            else -> {
                val msg = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE).orEmpty()
                val text = if (status == PackageInstaller.STATUS_FAILURE_ABORTED) "已取消安裝新版" else "新版安裝失敗：$msg"
                Repo.updateFailed(text)
            }
        }
    }
}

/** 自己被更新完（MY_PACKAGE_REPLACED）：發一則通知，點了打開 App。 */
class UpdatedReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        val version = runCatching { ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName }.getOrNull().orEmpty()
        Notifier.updated(ctx, version)
    }
}
