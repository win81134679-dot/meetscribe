package io.github.win81134679.meetscribe.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.core.content.IntentCompat
import io.github.win81134679.meetscribe.core.Notifier
import io.github.win81134679.meetscribe.core.Repo

class MainActivity : ComponentActivity() {
    /** 從其他 App 分享進來、還沒匯入的錄音。 */
    private var incoming by mutableStateOf<Uri?>(null)
    /** 從通知點進來：要打開的會議，或 "settings"。 */
    private var jump by mutableStateOf<String?>(null)

    private val askNotifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        Repo.init(this)
        incoming = sharedUri(intent)
        jump = jumpTarget(intent)
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        setContent {
            MeetScribeTheme {
                AppRoot(incoming, onIncomingHandled = { incoming = null }, jump, onJumpHandled = { jump = null })
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        sharedUri(intent)?.let { incoming = it }
        jumpTarget(intent)?.let { jump = it }
    }

    private fun jumpTarget(i: Intent?): String? =
        i?.getStringExtra(Notifier.EXTRA_OPEN_PROJECT) ?: i?.getStringExtra(Notifier.EXTRA_OPEN_SETTINGS)?.let { SETTINGS }

    private fun sharedUri(i: Intent?): Uri? = when (i?.action) {
        Intent.ACTION_SEND -> IntentCompat.getParcelableExtra(i, Intent.EXTRA_STREAM, Uri::class.java)
        Intent.ACTION_VIEW -> i.data
        else -> null
    }
}

private enum class Tab(val label: String, val icon: ImageVector) {
    New("新增轉錄", Ic.Add),
    Projects("會議記錄", Ic.List),
    Recordings("錄音管理", Ic.Mic),
    Settings("設定", Ic.Settings),
}

private const val SETTINGS = "settings"

@Composable
private fun AppRoot(incoming: Uri?, onIncomingHandled: () -> Unit, jump: String?, onJumpHandled: () -> Unit) {
    // 使用者定的入口：一律從「新增轉錄」開始
    var tab by rememberSaveable { mutableStateOf(Tab.New) }
    var openProject by rememberSaveable { mutableStateOf<String?>(null) }
    /** 錄音管理裡打開的那段錄音（[io.github.win81134679.meetscribe.core.RecordingItem.key]）；會議頁可以疊在它上面。 */
    var openRecording by rememberSaveable { mutableStateOf<String?>(null) }
    val snackbar = remember { SnackbarHostState() }

    LaunchedEffect(Unit) {
        Repo.toasts.collect { (_, text) -> snackbar.showSnackbar(text) }
    }
    LaunchedEffect(incoming) {
        if (incoming != null) {
            openProject = null
            openRecording = null
            tab = Tab.New
        }
    }
    LaunchedEffect(jump) {
        when (jump) {
            null -> return@LaunchedEffect
            SETTINGS -> { openProject = null; openRecording = null; tab = Tab.Settings }
            else -> openProject = jump
        }
        onJumpHandled()
    }
    BackHandler(enabled = openProject != null) { openProject = null }
    BackHandler(enabled = openProject == null && openRecording != null) { openRecording = null }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            if (openProject == null && openRecording == null) {
                NavigationBar {
                    for (t in Tab.entries) {
                        NavigationBarItem(
                            selected = tab == t,
                            onClick = { tab = t },
                            icon = { Icon(t.icon, contentDescription = null) },
                            label = { Text(t.label) },
                        )
                    }
                }
            }
        },
    ) { pad ->
        Box(Modifier.fillMaxSize().padding(pad)) {
            val err = Repo.initError
            when {
                err != null -> InitError(err)
                openProject != null -> ProjectScreen(openProject!!, onBack = { openProject = null })
                openRecording != null -> RecordingScreen(
                    openRecording!!,
                    onBack = { openRecording = null },
                    onOpenMeeting = { openProject = it },
                    onTranscribe = { item ->
                        useRecordingForNewJob(item)
                        openRecording = null
                        tab = Tab.New
                    },
                )
                tab == Tab.New -> NewJobScreen(
                    incoming = incoming,
                    onIncomingHandled = onIncomingHandled,
                    onStarted = { id -> openProject = id },
                    onOpenSettings = { tab = Tab.Settings },
                )
                tab == Tab.Projects -> ProjectsScreen(onOpen = { openProject = it })
                tab == Tab.Recordings -> RecordingsScreen(onOpen = { openRecording = it.key })
                else -> SettingsScreen()
            }
        }
    }
}

@Composable
private fun InitError(err: String) {
    SectionCard("無法啟動", "核心程式初始化失敗，請把下面這段訊息回報給開發者。", Modifier.padding(16.dp)) {
        Text(err)
    }
}
