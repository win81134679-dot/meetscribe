package io.github.win81134679.meetscribe.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.win81134679.meetscribe.core.ProjectSummary
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.core.formatClock

@Composable
fun ProjectsScreen(onOpen: (String) -> Unit) {
    val projects by Repo.projects.collectAsState()
    val progress by Repo.progress.collectAsState()
    val busy by Repo.busy.collectAsState()
    LaunchedEffect(Unit) { runCatching { Repo.refreshProjects() } }
    var query by rememberSaveable { mutableStateOf("") }

    if (projects.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
            Text("還沒有會議記錄。\n到「新增轉錄」選一個錄音檔開始。", color = Pal.current.textMuted)
        }
        return
    }
    LazyColumn(
        contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        item { Text("會議記錄", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.SemiBold) }
        // 會議多了才需要找：五場以上才出現搜尋框
        if (projects.size >= 5 || query.isNotEmpty()) {
            item { SearchField(query, "搜尋會議標題") { query = it } }
        }
        val q = query.trim()
        val shown = if (q.isEmpty()) projects else projects.filter { it.title.contains(q, ignoreCase = true) || it.createdAt.contains(q) }
        if (shown.isEmpty()) item { Text("沒有符合「$q」的會議。", color = Pal.current.textMuted) }
        items(shown, key = { it.id }) { p ->
            ProjectRow(p, p.id in busy, progress[p.id]?.let { it.stageLabel to it.fraction }) { onOpen(p.id) }
        }
    }
}

@Composable
private fun ProjectRow(p: ProjectSummary, busy: Boolean, prog: Pair<String, Float>?, onClick: () -> Unit) {
    val pal = Pal.current
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(pal.surface)
            .border(1.dp, pal.border, RoundedCornerShape(16.dp))
            .clickable(onClick = onClick)
            .padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                p.title, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Medium,
                maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.width(8.dp))
            val (label, status) = when {
                busy && p.status != "running" -> "排隊中" to "queued"
                busy -> "處理中" to "running"
                else -> p.statusLabel to p.status
            }
            Chip(label, statusColor(status))
        }
        Text(
            buildString {
                append(p.createdAt)
                if (p.duration > 0) append("　${formatClock(p.duration)}")
                if (p.speakers > 0) append("　${p.speakers} 位說話者")
                if (p.totalUsd > 0) append(if (p.totalUsd < 0.01) "　< US$0.01" else "　US$%.2f".format(p.totalUsd))
            },
            color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
        )
        if (busy) {
            if (prog != null) {
                LinearProgressIndicator(progress = { prog.second }, modifier = Modifier.fillMaxWidth())
                Text(prog.first, color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
            } else {
                LinearProgressIndicator(Modifier.fillMaxWidth())
            }
        } else if (p.error != null) {
            Text(p.error, color = pal.danger, style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }
}
