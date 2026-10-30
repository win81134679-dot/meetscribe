package io.github.win81134679.meetscribe.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.win81134679.meetscribe.core.Repo
import kotlinx.coroutines.launch

/** 白色圓角卡片（16dp），可帶標題與說明。 */
@Composable
fun SectionCard(
    title: String? = null,
    subtitle: String? = null,
    modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit,
) {
    val p = Pal.current
    Column(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(p.surface)
            .border(1.dp, p.border, RoundedCornerShape(16.dp))
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (title != null) Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = p.textMuted)
        content()
    }
}

@Composable
fun SwitchRow(label: String, checked: Boolean, hint: String? = null, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(enabled = enabled, role = Role.Switch) { onChange(!checked) }
            .padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.bodyLarge)
            if (hint != null) Text(hint, style = MaterialTheme.typography.bodySmall, color = Pal.current.textMuted)
        }
        Spacer(Modifier.width(12.dp))
        Switch(checked = checked, onCheckedChange = null, enabled = enabled)
    }
}

/** 一列「標籤：目前值」，點了跳出單選清單。 */
@Composable
fun SelectRow(label: String, options: List<Pair<String, String>>, selected: String, onSelect: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val current = options.firstOrNull { it.first == selected }?.second ?: selected
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .clickable(role = Role.DropdownList) { open = true }
            .padding(vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        Text(current, color = Pal.current.accentStrong, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
    }
    if (open) {
        ChoiceDialog(label, options, selected, onDismiss = { open = false }) {
            onSelect(it)
            open = false
        }
    }
}

@Composable
fun ChoiceDialog(
    title: String,
    options: List<Pair<String, String>>,
    selected: String?,
    onDismiss: () -> Unit,
    onSelect: (String) -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            LazyColumn(Modifier.heightIn(max = 420.dp)) {
                items(options, key = { it.first }) { (id, label) ->
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .clickable(role = Role.RadioButton) { onSelect(id) }
                            .padding(vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RadioButton(selected = id == selected, onClick = null)
                        Spacer(Modifier.width(10.dp))
                        Text(label)
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("關閉") } },
    )
}

@Composable
fun IconLabelButton(icon: ImageVector?, text: String, primary: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val inner: @Composable () -> Unit = {
        if (icon != null) {
            Icon(icon, contentDescription = null, Modifier.size(18.dp))
            Spacer(Modifier.width(6.dp))
        }
        Text(text)
    }
    if (primary) Button(onClick = onClick, enabled = enabled) { inner() }
    else OutlinedButton(onClick = onClick, enabled = enabled) { inner() }
}

@Composable
fun Chip(text: String, color: Color) {
    Box(
        Modifier
            .clip(RoundedCornerShape(50))
            .background(color.copy(alpha = 0.14f))
            .padding(horizontal = 10.dp, vertical = 3.dp),
    ) {
        Text(text, color = color, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Medium)
    }
}

@Composable
fun Dot(color: Color, size: Int = 10) {
    Box(Modifier.size(size.dp).clip(CircleShape).background(color))
}

@Composable
fun statusColor(status: String): Color {
    val p = Pal.current
    return when (status) {
        "done" -> p.success
        "failed" -> p.danger
        "cancelled" -> p.warning
        else -> p.accentStrong
    }
}

@Composable
fun Busy(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
        Spacer(Modifier.width(10.dp))
        Text(text, color = Pal.current.textMuted)
    }
}

/**
 * 金鑰輸入框：唯一一份實作，新增轉錄頁的設定卡與設定頁共用（同桌面 views::settings::key_box）。
 * 已存的金鑰只顯示遮罩後的樣子；輸入新的會覆蓋。
 */
@Composable
fun KeyBox(kind: String, label: String, placeholder: String, links: List<Pair<String, String>>) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val keys by Repo.keys.collectAsState()
    val tests by Repo.keyTests.collectAsState()
    val saved = keys[kind]
    var input by remember { mutableStateOf("") }
    var testing by remember { mutableStateOf(false) }
    val result = tests[kind]

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(label, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium)
        if (saved != null) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Dot(Pal.current.success, 8)
                Spacer(Modifier.width(8.dp))
                Text(saved, fontFamily = FontFamily.Monospace, modifier = Modifier.weight(1f))
            }
        }
        OutlinedTextField(
            value = input,
            onValueChange = { input = it.trim() },
            label = { Text(if (saved == null) "貼上金鑰" else "換一把新的金鑰") },
            placeholder = { Text(placeholder) },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
            modifier = Modifier.fillMaxWidth(),
        )
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            IconLabelButton(null, "儲存", primary = true, enabled = input.isNotEmpty()) {
                scope.launch {
                    if (Repo.saveKey(kind, input)) {
                        input = ""
                        testing = true
                        Repo.tryCall("test_key", "kind" to kind)
                    }
                }
            }
            if (saved != null) {
                IconLabelButton(null, "測試") {
                    testing = true
                    Repo.keyTests.value = Repo.keyTests.value - kind
                    scope.launch { Repo.tryCall("test_key", "kind" to kind) }
                }
                IconLabelButton(Ic.Delete, "刪除") { scope.launch { Repo.deleteKey(kind) } }
            }
            for ((text, url) in links) {
                TextButton(onClick = { Files.openUrl(ctx, url) }) { Text("$text ↗") }
            }
        }
        when {
            testing && result == null -> Busy("檢查中…")
            result != null -> Text(
                result.text,
                color = if (result.ok) Pal.current.success else Pal.current.danger,
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

/** 簡易 Markdown：標題、清單、粗體、行內程式碼、表格（原樣等寬顯示）。摘要用。 */
@Composable
fun MarkdownText(md: String, modifier: Modifier = Modifier) {
    val p = Pal.current
    Column(modifier, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        var inCode = false
        for (raw in md.lines()) {
            val line = raw.trimEnd()
            if (line.trimStart().startsWith("```")) {
                inCode = !inCode
                continue
            }
            when {
                inCode || line.trimStart().startsWith("|") -> Text(
                    line, fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = p.text,
                )
                line.isBlank() -> Spacer(Modifier.size(2.dp))
                line.startsWith("#") -> {
                    val level = line.takeWhile { it == '#' }.length
                    Text(
                        inline(line.drop(level).trim(), p.surfaceAlt),
                        style = when (level) {
                            1 -> MaterialTheme.typography.titleLarge
                            2 -> MaterialTheme.typography.titleMedium
                            else -> MaterialTheme.typography.titleSmall
                        },
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.padding(top = if (level <= 2) 8.dp else 4.dp),
                    )
                }
                Regex("""^\s*([-*+]|\d+[.)])\s+""").containsMatchIn(line) -> {
                    val indent = line.takeWhile { it == ' ' }.length / 2
                    val m = Regex("""^\s*([-*+]|\d+[.)])\s+""").find(line)!!
                    val bullet = m.groupValues[1].let { if (it.first().isDigit()) it else "•" }
                    val body = line.substring(m.range.last + 1)
                    Row(Modifier.padding(start = (indent * 16).dp)) {
                        Text(bullet, Modifier.width(22.dp), color = p.textMuted)
                        Text(inline(body, p.surfaceAlt), style = MaterialTheme.typography.bodyMedium)
                    }
                }
                line.startsWith(">") -> Text(
                    inline(line.trimStart('>', ' '), p.surfaceAlt),
                    color = p.textMuted, style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(start = 12.dp),
                )
                line.trim() == "---" -> Box(Modifier.fillMaxWidth().size(1.dp).background(p.border))
                else -> Text(inline(line, p.surfaceAlt), style = MaterialTheme.typography.bodyMedium)
            }
        }
    }
}

private fun inline(s: String, codeBg: Color): AnnotatedString = buildAnnotatedString {
    var i = 0
    while (i < s.length) {
        when {
            s.startsWith("**", i) && s.indexOf("**", i + 2) > 0 -> {
                val end = s.indexOf("**", i + 2)
                withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(s.substring(i + 2, end)) }
                i = end + 2
            }
            s[i] == '`' && s.indexOf('`', i + 1) > 0 -> {
                val end = s.indexOf('`', i + 1)
                withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = codeBg)) { append(s.substring(i + 1, end)) }
                i = end + 1
            }
            else -> {
                append(s[i])
                i++
            }
        }
    }
}
