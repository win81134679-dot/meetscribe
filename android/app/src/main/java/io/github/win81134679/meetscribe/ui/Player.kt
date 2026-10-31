package io.github.win81134679.meetscribe.ui

import android.media.MediaPlayer
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.github.win81134679.meetscribe.core.formatClock
import kotlinx.coroutines.delay
import java.io.File

/** 播放器（逐字稿頁、錄音管理共用）。`mp` 是 null：沒有音檔或打不開。 */
class Player(val mp: MediaPlayer?) {
    var playing by mutableStateOf(false)
    var position by mutableFloatStateOf(0f)
    var speed by mutableFloatStateOf(1f)
        private set
    val duration: Float get() = (mp?.duration ?: 0) / 1000f

    fun playFrom(secs: Double) {
        seek(secs.toFloat())
        start()
    }

    fun toggle() {
        val m = mp ?: return
        if (m.isPlaying) {
            m.pause()
            playing = false
        } else {
            start()
        }
    }

    fun seek(secs: Float) {
        val m = mp ?: return
        val s = secs.coerceIn(0f, duration.coerceAtLeast(0f))
        m.seekTo((s * 1000).toLong(), MediaPlayer.SEEK_CLOSEST)
        position = s
    }

    fun skip(delta: Float) = seek(position + delta)

    /** 1× → 1.25× → 1.5× → 2× → 0.75× → 1×。 */
    fun nextSpeed() {
        speed = SPEEDS[(SPEEDS.indexOf(speed) + 1) % SPEEDS.size]
        if (mp?.isPlaying == true) applySpeed()
    }

    fun pause() {
        mp?.takeIf { it.isPlaying }?.pause()
        playing = false
    }

    private fun start() {
        val m = mp ?: return
        m.start()
        // 暫停中設定速度會讓它開始播，所以只在播放時套用
        applySpeed()
        playing = m.isPlaying
    }

    private fun applySpeed() {
        val m = mp ?: return
        runCatching { m.playbackParams = m.playbackParams.setSpeed(speed) }
    }

    companion object {
        val SPEEDS = listOf(1f, 1.25f, 1.5f, 2f, 0.75f)
    }
}

/** 換了音檔就換一個播放器；`autoPlay` 時準備好就開始播（錄音管理按播放鍵）。 */
@Composable
fun rememberPlayer(path: String?, autoPlay: Boolean = false): Player {
    val player = remember(path) {
        Player(path?.takeIf { File(it).isFile }?.let { runCatching { MediaPlayer().apply { setDataSource(it); prepare() } }.getOrNull() })
    }
    DisposableEffect(player) {
        player.mp?.setOnCompletionListener { player.playing = false }
        onDispose { player.mp?.release() }
    }
    LaunchedEffect(player) {
        if (autoPlay) player.toggle()
        while (true) {
            player.mp?.let { if (player.playing) player.position = it.currentPosition / 1000f }
            delay(250)
        }
    }
    return player
}

private fun speedLabel(v: Float) = (if (v % 1f == 0f) "%.0f".format(v) else "%s".format(v)) + "×"

/** 播放列（一行）：播放／暫停、進度條、目前時間、速度。逐字稿頁上方、錄音管理底部的迷你播放器用。 */
@Composable
fun PlayerBar(player: Player, modifier: Modifier = Modifier) {
    val pal = Pal.current
    Row(modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = { player.toggle() }, enabled = player.mp != null) {
            Icon(if (player.playing) Ic.Pause else Ic.Play, contentDescription = if (player.playing) "暫停" else "播放")
        }
        Slider(
            value = player.position.coerceIn(0f, player.duration.coerceAtLeast(1f)),
            onValueChange = { player.seek(it) },
            valueRange = 0f..player.duration.coerceAtLeast(1f),
            enabled = player.mp != null,
            modifier = Modifier.weight(1f),
        )
        Text(
            formatClock(player.position.toDouble()),
            style = MaterialTheme.typography.bodySmall, color = pal.textMuted, fontFamily = FontFamily.Monospace,
            modifier = Modifier.padding(horizontal = 4.dp),
        )
        TextButton(onClick = { player.nextSpeed() }, enabled = player.mp != null) { Text(speedLabel(player.speed)) }
    }
}

/** 完整播放器（錄音詳細頁）：進度條＋前後時間，下面一排倒退 15 秒、大的播放鍵、快轉 15 秒、速度。 */
@Composable
fun PlayerPanel(player: Player) {
    val pal = Pal.current
    Column(Modifier.fillMaxWidth()) {
        Slider(
            value = player.position.coerceIn(0f, player.duration.coerceAtLeast(1f)),
            onValueChange = { player.seek(it) },
            valueRange = 0f..player.duration.coerceAtLeast(1f),
            enabled = player.mp != null,
        )
        Row(Modifier.fillMaxWidth()) {
            Text(formatClock(player.position.toDouble()), color = pal.textMuted, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
            Spacer(Modifier.weight(1f))
            Text(formatClock(player.duration.toDouble()), color = pal.textMuted, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = { player.skip(-15f) }, enabled = player.mp != null) { Text("−15 秒") }
            Spacer(Modifier.width(12.dp))
            FilledIconButton(onClick = { player.toggle() }, enabled = player.mp != null, modifier = Modifier.size(56.dp)) {
                Icon(if (player.playing) Ic.Pause else Ic.Play, contentDescription = if (player.playing) "暫停" else "播放", Modifier.size(30.dp))
            }
            Spacer(Modifier.width(12.dp))
            TextButton(onClick = { player.skip(15f) }, enabled = player.mp != null) { Text("+15 秒") }
            Spacer(Modifier.width(4.dp))
            TextButton(onClick = { player.nextSpeed() }, enabled = player.mp != null) { Text(speedLabel(player.speed)) }
        }
    }
}
