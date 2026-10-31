//! 匯出。每種格式一個 [`Exporter`]，加新格式只要在 [`registry`] 多登記一個。
//! 要匯出哪些格式、放哪、檔名、內容怎麼排，全部在 [`ExportOptions`]（設定頁、新增轉錄、匯出頁、CLI 共用）。

use crate::model::Project;
use crate::textutil::{fmt_duration_zh, fmt_hms, fmt_subtitle, safe_filename};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub trait Exporter: Send + Sync {
    /// 設定檔裡記的代號。
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn extension(&self) -> &'static str;
    /// 這個格式會放摘要（[`Content`] 對它有作用）。
    fn has_summary(&self) -> bool {
        false
    }
    /// 只放摘要（Word、PDF；2026-10-31 使用者：「docx更新成不放逐字稿」）：[`Content`] 對它沒作用，沒有摘要就不產這個檔。
    fn summary_only(&self) -> bool {
        false
    }
    fn render(&self, project: &Project, opts: &ExportOptions) -> Result<Vec<u8>>;
}

/// 摘要與逐字稿怎麼放（只影響 Markdown、純文字；Word、PDF 固定只放摘要，字幕、CSV、JSON 固定是逐字稿／完整資料）。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Content {
    /// 一個檔：摘要在前、逐字稿在後。
    #[default]
    Both,
    /// 兩個檔：「<名稱> 摘要」「<名稱> 逐字稿」。
    Separate,
    TranscriptOnly,
    SummaryOnly,
}

impl Content {
    pub const ALL: [Content; 4] = [Content::Both, Content::Separate, Content::TranscriptOnly, Content::SummaryOnly];
    pub fn label(self) -> &'static str {
        match self {
            Content::Both => "摘要＋逐字稿（同一個檔）",
            Content::Separate => "摘要、逐字稿分成兩個檔",
            Content::TranscriptOnly => "只要逐字稿",
            Content::SummaryOnly => "只要摘要",
        }
    }
    fn summary(self) -> bool {
        !matches!(self, Content::TranscriptOnly)
    }
    fn transcript(self) -> bool {
        !matches!(self, Content::SummaryOnly)
    }
}

/// 檔名樣式可用的代號。
pub const FILENAME_TOKENS: &str = "{標題}　{日期}　{時間}";
/// 常用檔名樣式（設定頁的下拉選單）。
pub const FILENAME_PRESETS: [&str; 3] = ["{標題}", "{日期} {標題}", "{日期}_{時間} {標題}"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ExportOptions {
    /// 處理完成後自動匯出。
    pub auto: bool,
    /// [`Exporter::id`] 清單。
    pub formats: Vec<String>,
    /// 存放資料夾；空白＝原始錄音所在資料夾。
    pub dir: String,
    /// 每場會議另建一個子資料夾（名稱同檔名）。預設開（2026-10-31 使用者）。
    pub subfolder: bool,
    /// 檔名樣式，可用 [`FILENAME_TOKENS`]。
    pub filename: String,
    /// 同名檔案直接覆蓋；否則另存成「名稱 (2)」。
    pub overwrite: bool,
    pub content: Content,
    /// 逐字稿每段前面加 [時:分:秒]。
    pub timestamps: bool,
    /// 逐字稿與字幕標出說話者。
    pub speaker_names: bool,
    /// 同一人連續發言併成一段（關掉則一句一行）。
    pub merge_turns: bool,
    /// 開頭附檔案資訊（原始檔、長度、模型）與發言者統計表。
    pub header: bool,
    /// 會議資料夾也放一份錄音（`<檔名>.m4a` 等；2026-10-31 使用者）。只有 Android 用：電腦版的錄音本來就在電腦上。
    pub audio: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            auto: true,
            // PDF 預設勾（2026-10-31 使用者：「輸出檔案也加入PDF並預設勾選」）
            formats: vec!["docx".into(), "pdf".into(), "md".into(), "srt".into()],
            dir: String::new(),
            subfolder: true,
            filename: FILENAME_PRESETS[0].into(),
            overwrite: false,
            content: Content::Both,
            timestamps: true,
            speaker_names: true,
            merge_turns: true,
            header: true,
            audio: true,
        }
    }
}

impl ExportOptions {
    /// 檔名（不含副檔名）：樣式代入標題與建立時間，去掉 Windows 不允許的字元。
    pub fn file_stem(&self, p: &Project) -> String {
        let template = match self.filename.trim() {
            "" => FILENAME_PRESETS[0],
            t => t,
        };
        safe_filename(
            &template
                .replace("{標題}", &p.title)
                .replace("{日期}", &p.created_at.format("%Y-%m-%d").to_string())
                .replace("{時間}", &p.created_at.format("%H%M").to_string()),
        )
    }

    /// 實際寫入的資料夾。
    pub fn target_dir(&self, p: &Project) -> PathBuf {
        let base = if self.dir.trim().is_empty() {
            p.source_file
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            PathBuf::from(self.dir.trim())
        };
        if self.subfolder { base.join(self.file_stem(p)) } else { base }
    }

    /// 只保留認得的格式代號，順序跟 [`registry`] 一致。
    pub fn known_formats(&self) -> Vec<&'static str> {
        registry()
            .iter()
            .map(|e| e.id())
            .filter(|id| self.formats.iter().any(|f| f == id))
            .collect()
    }
}

pub fn registry() -> Vec<Box<dyn Exporter>> {
    vec![
        Box::new(MarkdownExporter),
        Box::new(DocxExporter),
        Box::new(PdfExporter),
        Box::new(TextExporter),
        Box::new(SrtExporter),
        Box::new(VttExporter),
        Box::new(CsvExporter),
        Box::new(JsonExporter),
    ]
}

/// 依選項把選定的格式寫到 [`ExportOptions::target_dir`]，回傳寫出的檔案。
pub fn export_project(project: &Project, opts: &ExportOptions) -> Result<Vec<PathBuf>> {
    let ids = opts.known_formats();
    if ids.is_empty() {
        return Err(Error::Other("沒有選任何匯出格式".into()));
    }
    if project.summary.is_none() {
        // 沒有摘要時，只放摘要的格式（Word、PDF，以及「只要摘要」下的 Markdown、純文字）都不會產檔
        let empty = |e: &dyn Exporter| e.summary_only() || (e.has_summary() && opts.content == Content::SummaryOnly);
        if registry().iter().filter(|e| ids.contains(&e.id())).all(|e| empty(e.as_ref())) {
            return Err(Error::Other(if opts.content == Content::SummaryOnly {
                "這場會議還沒有摘要，「只要摘要」沒有東西可匯出".into()
            } else {
                "這場會議還沒有摘要；Word、PDF 只放摘要，請另外勾選 Markdown 或字幕檔".into()
            }));
        }
    }
    let dir = opts.target_dir(project);
    std::fs::create_dir_all(&dir)?;
    let base = opts.file_stem(project);
    let mut written = Vec::new();
    for ex in registry().into_iter().filter(|e| ids.contains(&e.id())) {
        // (檔名後綴, 這個檔的內容選項)
        let parts: Vec<(&str, Content)> = match (ex.has_summary(), opts.content) {
            _ if ex.summary_only() && project.summary.is_none() => continue,
            _ if ex.summary_only() => vec![("", Content::SummaryOnly)],
            (true, Content::Separate) if project.summary.is_some() => {
                vec![(" 摘要", Content::SummaryOnly), (" 逐字稿", Content::TranscriptOnly)]
            }
            (true, Content::Separate) => vec![(" 逐字稿", Content::TranscriptOnly)],
            (true, Content::SummaryOnly) if project.summary.is_none() => continue,
            (true, c) => vec![("", c)],
            (false, _) => vec![("", Content::TranscriptOnly)],
        };
        for (suffix, content) in parts {
            let part = ExportOptions {
                content,
                ..opts.clone()
            };
            let bytes = ex.render(project, &part)?;
            let name = format!("{base}{suffix}");
            let path = if opts.overwrite {
                dir.join(format!("{name}.{}", ex.extension()))
            } else {
                unique_path(&dir, &name, ex.extension())
            };
            std::fs::write(&path, bytes)?;
            written.push(path);
        }
    }
    Ok(written)
}

fn unique_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{base}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{base} ({n}).{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded")
}

/// 逐字稿的一句：起訖時間、說話者名稱（不標說話者時是空字串）、文字。
fn rows<'a>(project: &'a Project, opts: &ExportOptions) -> Vec<(f64, f64, String, &'a str)> {
    project
        .segments
        .iter()
        .map(|s| {
            let name = if opts.speaker_names && s.speaker.is_some() {
                project.speaker_name(s.speaker.as_deref())
            } else {
                String::new()
            };
            (s.start, s.end, name, s.text.as_str())
        })
        .collect()
}

/// 閱讀用格式的段落：(起始時間, 說話者名稱, 文字)。`merge_turns` 時同一人連續發言併成一段。
fn turns(project: &Project, opts: &ExportOptions) -> Vec<(f64, String, String)> {
    let mut out: Vec<(f64, String, String)> = Vec::new();
    let mut last: Option<Option<String>> = None;
    for s in &project.segments {
        if opts.merge_turns
            && last.as_ref() == Some(&s.speaker)
            && let Some(t) = out.last_mut()
        {
            if crate::model::needs_space(&t.2, &s.text) {
                t.2.push(' ');
            }
            t.2.push_str(&s.text);
            continue;
        }
        let name = if opts.speaker_names && s.speaker.is_some() {
            project.speaker_name(s.speaker.as_deref())
        } else {
            String::new()
        };
        out.push((s.start, name, s.text.clone()));
        last = Some(s.speaker.clone());
    }
    out
}

fn meta_lines(project: &Project) -> Vec<String> {
    // Android 的原始檔是 App 內部的匯入副本，完整路徑對使用者沒意義，只留檔名
    #[cfg(target_os = "android")]
    let source = project
        .source_file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    #[cfg(not(target_os = "android"))]
    let source = project.source_file.display().to_string();
    let mut v = vec![
        format!("原始檔案：{source}"),
        format!("長度：{}", fmt_duration_zh(project.duration)),
        format!("建立時間：{}", project.created_at.format("%Y-%m-%d %H:%M")),
    ];
    if !project.speakers.is_empty() {
        v.push(format!("發言人數：{}", project.speakers.len()));
    }
    v.push(format!("轉錄模型：{}", project.models.stt));
    v
}

/// 文件標題：只放摘要或只放逐字稿時註明。
fn doc_title(p: &Project, opts: &ExportOptions) -> String {
    match opts.content {
        Content::SummaryOnly => format!("{} 摘要", p.title),
        Content::TranscriptOnly if p.summary.is_some() => format!("{} 逐字稿", p.title),
        _ => p.title.clone(),
    }
}

pub struct MarkdownExporter;
impl Exporter for MarkdownExporter {
    fn id(&self) -> &'static str {
        "md"
    }
    fn label(&self) -> &'static str {
        "Markdown"
    }
    fn extension(&self) -> &'static str {
        "md"
    }
    fn has_summary(&self) -> bool {
        true
    }
    fn render(&self, p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        let mut s = format!("# {}\n\n", doc_title(p, o));
        if o.header {
            for m in meta_lines(p) {
                s.push_str(&format!("- {m}\n"));
            }
        }
        if o.content.summary()
            && let Some(sum) = &p.summary
        {
            s.push_str("\n## 會議摘要\n\n");
            s.push_str(&demote_headings(&sum.markdown, 2));
            s.push('\n');
        }
        if o.content.transcript() {
            if o.header && !p.speakers.is_empty() {
                s.push_str("\n## 發言者\n\n| 說話者 | 發言時間 | 句數 |\n|---|---|---|\n");
                for (id, secs, count) in p.speaker_stats() {
                    s.push_str(&format!(
                        "| {} | {} | {} |\n",
                        p.speaker_name(Some(&id)),
                        fmt_duration_zh(secs),
                        count
                    ));
                }
            }
            s.push_str("\n## 逐字稿\n\n");
            for (start, name, text) in turns(p, o) {
                if o.timestamps {
                    s.push_str(&format!("`{}` ", fmt_hms(start)));
                }
                if !name.is_empty() {
                    s.push_str(&format!("**{name}**："));
                }
                s.push_str(&text);
                s.push_str("\n\n");
            }
        }
        Ok(s.into_bytes())
    }
}

/// 摘要裡的 `#` 往下降，避免和外層標題同級。
fn demote_headings(md: &str, by: usize) -> String {
    md.lines()
        .map(|l| {
            if l.starts_with('#') {
                format!("{}{}", "#".repeat(by), l)
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 摘要的 Markdown 轉成純文字：拿掉標題井號、粗體、程式碼記號，表格分隔列略過。
fn plain_summary(md: &str) -> String {
    md.lines()
        .filter(|l| {
            let t = l.trim();
            !(t.starts_with('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')))
        })
        .map(|l| {
            l.trim_start_matches('#')
                .trim_start_matches(' ')
                .replace("**", "")
                .replace('`', "")
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub struct TextExporter;
impl Exporter for TextExporter {
    fn id(&self) -> &'static str {
        "txt"
    }
    fn label(&self) -> &'static str {
        "純文字"
    }
    fn extension(&self) -> &'static str {
        "txt"
    }
    fn has_summary(&self) -> bool {
        true
    }
    fn render(&self, p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        let mut s = String::from("\u{feff}");
        if o.header {
            s.push_str(&format!("{}\r\n", doc_title(p, o)));
            for m in meta_lines(p) {
                s.push_str(&format!("{m}\r\n"));
            }
            s.push_str("\r\n");
        }
        let summary = o.content.summary().then_some(p.summary.as_ref()).flatten();
        if let Some(sum) = summary {
            s.push_str("【會議摘要】\r\n\r\n");
            s.push_str(&plain_summary(&sum.markdown));
            s.push_str("\r\n\r\n");
        }
        if o.content.transcript() {
            if summary.is_some() {
                s.push_str("【逐字稿】\r\n\r\n");
            }
            for (start, name, text) in turns(p, o) {
                if o.timestamps {
                    s.push_str(&format!("[{}] ", fmt_hms(start)));
                }
                if !name.is_empty() {
                    s.push_str(&format!("{name}："));
                }
                s.push_str(&text);
                s.push_str(if o.merge_turns { "\r\n\r\n" } else { "\r\n" });
            }
        }
        Ok(s.into_bytes())
    }
}

pub struct SrtExporter;
impl Exporter for SrtExporter {
    fn id(&self) -> &'static str {
        "srt"
    }
    fn label(&self) -> &'static str {
        "SRT 字幕"
    }
    fn extension(&self) -> &'static str {
        "srt"
    }
    fn render(&self, p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        let mut s = String::new();
        for (i, (start, end, name, text)) in rows(p, o).into_iter().enumerate() {
            let line = if name.is_empty() {
                text.to_string()
            } else {
                format!("{name}：{text}")
            };
            s.push_str(&format!(
                "{}\n{} --> {}\n{}\n\n",
                i + 1,
                fmt_subtitle(start, ','),
                fmt_subtitle(end.max(start + 0.5), ','),
                line
            ));
        }
        Ok(s.into_bytes())
    }
}

pub struct VttExporter;
impl Exporter for VttExporter {
    fn id(&self) -> &'static str {
        "vtt"
    }
    fn label(&self) -> &'static str {
        "WebVTT 字幕"
    }
    fn extension(&self) -> &'static str {
        "vtt"
    }
    fn render(&self, p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        let mut s = String::from("WEBVTT\n\n");
        for (start, end, name, text) in rows(p, o) {
            let line = if name.is_empty() {
                text.to_string()
            } else {
                format!("<v {}>{}", name.replace('>', "＞"), text)
            };
            s.push_str(&format!(
                "{} --> {}\n{}\n\n",
                fmt_subtitle(start, '.'),
                fmt_subtitle(end.max(start + 0.5), '.'),
                line
            ));
        }
        Ok(s.into_bytes())
    }
}

pub struct CsvExporter;
impl Exporter for CsvExporter {
    fn id(&self) -> &'static str {
        "csv"
    }
    fn label(&self) -> &'static str {
        "CSV（Excel 可開）"
    }
    fn extension(&self) -> &'static str {
        "csv"
    }
    fn render(&self, p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut s = String::from("\u{feff}開始,結束,說話者,內容\r\n");
        for (start, end, name, text) in rows(p, o) {
            s.push_str(&format!(
                "{},{},{},{}\r\n",
                fmt_hms(start),
                fmt_hms(end),
                q(&name),
                q(text)
            ));
        }
        Ok(s.into_bytes())
    }
}

pub struct JsonExporter;
impl Exporter for JsonExporter {
    fn id(&self) -> &'static str {
        "json"
    }
    fn label(&self) -> &'static str {
        "JSON（完整資料）"
    }
    fn extension(&self) -> &'static str {
        "json"
    }
    fn render(&self, p: &Project, _: &ExportOptions) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(p)?)
    }
}

pub struct DocxExporter;
impl Exporter for DocxExporter {
    fn id(&self) -> &'static str {
        "docx"
    }
    fn label(&self) -> &'static str {
        "Word 摘要"
    }
    fn extension(&self) -> &'static str {
        "docx"
    }
    fn summary_only(&self) -> bool {
        true
    }
    fn render(&self, p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        docx::render(p, &summary_only(o))
    }
}

/// PDF：跟 Word 同一份內容與版面（見 [`pdf`]）。
pub struct PdfExporter;
impl Exporter for PdfExporter {
    fn id(&self) -> &'static str {
        "pdf"
    }
    fn label(&self) -> &'static str {
        "PDF 摘要"
    }
    fn extension(&self) -> &'static str {
        "pdf"
    }
    fn summary_only(&self) -> bool {
        true
    }
    fn render(&self, p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        pdf::render(p, &summary_only(o))
    }
}

fn summary_only(o: &ExportOptions) -> ExportOptions {
    ExportOptions {
        content: Content::SummaryOnly,
        ..o.clone()
    }
}

/// 摘要 Markdown 的一個區塊（Word 與 PDF 共用同一份解析，兩邊版面才會一樣）。
enum MdBlock<'a> {
    /// 表格的每一列（分隔列已去掉），第一列是表頭。
    Table(Vec<Vec<String>>),
    Heading(usize, &'a str),
    /// 條列：符號（•、☐、☑、「1.」）、縮排層級、內容。
    Item(String, usize, &'a str),
    Quote(&'a str),
    Para(&'a str),
}

/// 摘要第一行常是跟文件標題一樣的 `# 標題`，重複的略過；空行與分隔線略過。
fn md_blocks<'a>(md: &'a str, title: &str) -> Vec<MdBlock<'a>> {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut first_heading = true;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with('|') {
            let mut rows: Vec<Vec<String>> = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                let cells: Vec<String> = lines[i]
                    .trim()
                    .trim_matches('|')
                    .split('|')
                    .map(|c| c.trim().to_string())
                    .collect();
                let is_sep = cells
                    .iter()
                    .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
                if !is_sep {
                    rows.push(cells);
                }
                i += 1;
            }
            if rows.iter().any(|r| !r.is_empty()) {
                out.push(MdBlock::Table(rows));
            }
            continue;
        }
        if let Some(level) = heading_level(trimmed) {
            let text = trimmed.trim_start_matches('#').trim();
            let duplicate = first_heading && level == 1 && text.replace("**", "") == title;
            first_heading = false;
            if !duplicate {
                out.push(MdBlock::Heading(level, text));
            }
        } else if let Some((marker, rest)) = list_item(trimmed) {
            out.push(MdBlock::Item(marker, (line.len() - trimmed.len()) / 2, rest));
        } else if let Some(q) = trimmed.strip_prefix('>') {
            out.push(MdBlock::Quote(q.trim()));
        } else if !(trimmed.is_empty() || trimmed.chars().all(|c| matches!(c, '-' | '*' | '_'))) {
            out.push(MdBlock::Para(trimmed));
        }
        i += 1;
    }
    out
}

fn heading_level(s: &str) -> Option<usize> {
    let n = s.chars().take_while(|c| *c == '#').count();
    (n > 0 && s.chars().nth(n) == Some(' ')).then_some(n)
}

/// 條列：(符號, 內容)。待辦用方框、編號保留原編號、其他用圓點。
fn list_item(s: &str) -> Option<(String, &str)> {
    for (p, m) in [("- [ ] ", "☐"), ("- [x] ", "☑"), ("- [X] ", "☑"), ("* [ ] ", "☐"), ("* [x] ", "☑")] {
        if let Some(r) = s.strip_prefix(p) {
            return Some((m.to_string(), r));
        }
    }
    for p in ["- ", "* ", "+ "] {
        if let Some(r) = s.strip_prefix(p) {
            return Some(("•".to_string(), r));
        }
    }
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(r) = s[digits..].strip_prefix(". ")
    {
        return Some((format!("{}.", &s[..digits]), r));
    }
    None
}

/// `**粗體**` 切成 (文字, 是否粗體)，反引號拿掉。
fn inline_parts(text: &str) -> Vec<(String, bool)> {
    text.split("**")
        .enumerate()
        .filter(|(_, part)| !part.is_empty())
        .map(|(i, part)| (part.replace('`', ""), i % 2 == 1))
        .collect()
}

mod pdf;

/// Word 會議紀錄（2026-10-31 使用者：「docx也要修 我希望把排版...等變得更好」）。
/// A4、頁首會議名稱、頁尾頁碼；標題區＋會議資訊表；章節標題主色加底線；摘要的條列、待辦方框、編號、引言、表格各有樣式；
/// 發言者表附顏色與佔比。2026-10-31 起只放摘要（「docx更新成不放逐字稿」），逐字稿在 Markdown、字幕檔；PDF 照這份版面畫（[`pdf`]）。
mod docx {
    use super::*;
    use docx_rs::*;

    pub(super) const FONT_EA: &str = "微軟正黑體";
    const FONT_LATIN: &str = "Calibri";
    /// 主色（同程式介面的薰衣草紫）與深一階的標題色。
    pub(super) const ACCENT: &str = "7F77DD";
    pub(super) const ACCENT_DARK: &str = "4B4594";
    pub(super) const ACCENT_TINT: &str = "EEECFB";
    pub(super) const TEXT: &str = "1F2430";
    pub(super) const MUTED: &str = "6B7280";
    pub(super) const RULE: &str = "E3E1EE";
    /// 說話者顏色（印出來也看得清楚的深色版，順序同介面）。
    pub(super) const SPEAKER_COLORS: [&str; 10] = [
        "5B54C4", "0E7C66", "C2410C", "B4237A", "1D63C9", "7C3AED", "3F7D1F", "A16207", "0F766E", "BE123C",
    ];
    /// A4，單位 twip（1/1440 英吋）。
    const PAGE_W: u32 = 11906;
    const PAGE_H: u32 = 16838;
    const MARGIN: i32 = 1247; // 2.2 cm
    const BODY_W: usize = (PAGE_W as usize) - 2 * (MARGIN as usize);

    fn speaker_color(p: &Project, id: Option<&str>) -> &'static str {
        let idx = id.and_then(|id| p.speaker(id)).map(|s| s.color).unwrap_or(0);
        SPEAKER_COLORS[idx % SPEAKER_COLORS.len()]
    }

    /// 「1 小時 2 分」→「1 小時 2 分」不變；中文排版裡數字與單位之間不需要額外空白。
    fn duration(sec: f64) -> String {
        fmt_duration_zh(sec).replace(' ', "")
    }

    fn spacing(before: u32, after: u32) -> LineSpacing {
        LineSpacing::new().before(before).after(after)
    }

    fn bottom_rule(mut p: Paragraph, color: &str, size: usize) -> Paragraph {
        p.property = p.property.set_borders(
            ParagraphBorders::with_empty()
                .set(ParagraphBorder::new(ParagraphBorderPosition::Bottom).val(BorderType::Single).size(size).space(4).color(color)),
        );
        p
    }

    /// 圓點一律用中文字型畫（英文名字前的 ● 才不會變小）。
    fn dot(color: &str, size: usize) -> Run {
        Run::new()
            .add_text("● ")
            .fonts(RunFonts::new().east_asia(FONT_EA).ascii(FONT_EA).hi_ansi(FONT_EA))
            .color(color)
            .size(size)
    }

    fn styles(doc: Docx) -> Docx {
        // 章節（會議摘要／發言者／逐字稿）：主色、細底線
        let mut h1 = Style::new("Heading1", StyleType::Paragraph)
            .name("heading 1")
            .size(32)
            .bold()
            .color(ACCENT_DARK)
            .line_spacing(spacing(360, 160))
            .outline_lvl(0);
        h1.paragraph_property = h1.paragraph_property.set_borders(
            ParagraphBorders::with_empty()
                .set(ParagraphBorder::new(ParagraphBorderPosition::Bottom).val(BorderType::Single).size(6).space(4).color(RULE)),
        );
        let h1 = h1;
        // 頁首頁尾：頁碼欄位跟著段落樣式的字級與顏色
        doc.add_style(Style::new("Footer", StyleType::Paragraph).name("footer").size(16).color(MUTED))
            .add_style(h1)
            .add_style(
                Style::new("Heading2", StyleType::Paragraph)
                    .name("heading 2")
                    .size(28)
                    .bold()
                    .color(TEXT)
                    .line_spacing(spacing(280, 100))
                    .outline_lvl(1),
            )
            .add_style(
                Style::new("Heading3", StyleType::Paragraph)
                    .name("heading 3")
                    .size(24)
                    .bold()
                    .color(ACCENT_DARK)
                    .line_spacing(spacing(240, 80))
                    .outline_lvl(2),
            )
            .add_style(
                Style::new("Heading4", StyleType::Paragraph)
                    .name("heading 4")
                    .size(22)
                    .bold()
                    .color(TEXT)
                    .line_spacing(spacing(200, 60))
                    .outline_lvl(3),
            )
    }

    pub fn render(p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        let title = doc_title(p, o);
        let mut doc = Docx::new()
            .page_size(PAGE_W, PAGE_H)
            .page_margin(
                PageMargin::new()
                    .top(MARGIN)
                    .bottom(MARGIN)
                    .left(MARGIN)
                    .right(MARGIN)
                    .header(620)
                    .footer(620),
            )
            .default_fonts(RunFonts::new().east_asia(FONT_EA).ascii(FONT_LATIN).hi_ansi(FONT_LATIN))
            .default_size(22)
            .default_line_spacing(LineSpacing::new().line(312).line_rule(LineSpacingType::Auto).after(80))
            .header(Header::new().add_paragraph(
                Paragraph::new()
                    .style("Footer")
                    .align(AlignmentType::Right)
                    .add_run(Run::new().add_text(&p.title)),
            ))
            .footer(Footer::new().add_paragraph(
                Paragraph::new()
                    .style("Footer")
                    .align(AlignmentType::Center)
                    .add_run(Run::new().add_text("第 "))
                    .add_page_num(PageNum::new())
                    .add_run(Run::new().add_text(" 頁／共 "))
                    .add_num_pages(NumPages::new())
                    .add_run(Run::new().add_text(" 頁　·　語音轉錄")),
            ));
        doc = styles(doc);

        // 標題區
        let kind = match o.content {
            Content::SummaryOnly => "會議摘要",
            Content::TranscriptOnly => "會議逐字稿",
            _ => "會議紀錄",
        };
        doc = doc.add_paragraph(
            Paragraph::new()
                .line_spacing(spacing(0, 40))
                .add_run(Run::new().add_text(kind).size(20).bold().color(ACCENT).character_spacing(40)),
        );
        doc = doc.add_paragraph(
            Paragraph::new()
                .line_spacing(spacing(0, 60))
                .add_run(Run::new().add_text(&title).size(48).bold().color(TEXT)),
        );
        let mut sub = vec![p.created_at.format("%Y-%m-%d %H:%M").to_string()];
        if p.duration > 0.0 {
            sub.push(duration(p.duration));
        }
        if !p.speakers.is_empty() {
            sub.push(format!("{} 位發言者", p.speakers.len()));
        }
        doc = doc.add_paragraph(bottom_rule(
            Paragraph::new()
                .line_spacing(spacing(0, 240))
                .add_run(Run::new().add_text(sub.join("　·　")).size(20).color(MUTED)),
            ACCENT,
            12,
        ));
        if o.header {
            doc = doc.add_table(info_table(p));
        }

        if let Some(sum) = &p.summary {
            doc = doc.add_paragraph(heading("會議摘要"));
            doc = markdown_into(doc, &sum.markdown, &p.title);
        }
        if o.header && !p.speakers.is_empty() {
            doc = doc.add_paragraph(heading("發言者"));
            doc = doc.add_table(speaker_table(p));
        }
        pack(doc)
    }

    fn heading(text: &str) -> Paragraph {
        Paragraph::new().style("Heading1").keep_next(true).add_run(Run::new().add_text(text))
    }

    fn pack(doc: Docx) -> Result<Vec<u8>> {
        let mut buf = std::io::Cursor::new(Vec::new());
        doc.build()
            .pack(&mut buf)
            .map_err(|e| Error::Other(format!("產生 Word 檔失敗：{e}")))?;
        Ok(buf.into_inner())
    }

    fn light_borders() -> TableBorders {
        let b = |pos| TableBorder::new(pos).border_type(BorderType::Single).size(4).color(RULE);
        TableBorders::with_empty()
            .set(b(TableBorderPosition::Top))
            .set(b(TableBorderPosition::Bottom))
            .set(b(TableBorderPosition::InsideH))
            .set(b(TableBorderPosition::Left))
            .set(b(TableBorderPosition::Right))
            .set(b(TableBorderPosition::InsideV))
    }

    fn table(rows: Vec<TableRow>, grid: Vec<usize>) -> Table {
        Table::new(rows)
            .set_grid(grid)
            .width(BODY_W, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .set_borders(light_borders())
            .margins(TableCellMargins::new().margin(50, 120, 50, 120))
    }

    fn cell(runs: Vec<Run>, width: usize, fill: Option<&str>) -> TableCell {
        let mut para = Paragraph::new().line_spacing(spacing(0, 0));
        for r in runs {
            para = para.add_run(r);
        }
        let mut c = TableCell::new()
            .add_paragraph(para)
            .width(width, WidthType::Dxa)
            .vertical_align(VAlignType::Center);
        if let Some(f) = fill {
            c = c.shading(Shading::new().fill(f));
        }
        c
    }

    /// 會議資訊：左欄淡紫底的欄名、右欄內容。日期、長度、人數已在標題下，這裡放來源與用了哪些模型。
    fn info_table(p: &Project) -> Table {
        let key_w = 1900;
        let val_w = BODY_W - key_w;
        let source = meta_lines(p)
            .into_iter()
            .find_map(|l| l.strip_prefix("原始檔案：").map(str::to_string))
            .unwrap_or_default();
        let mut items = vec![("原始檔案".to_string(), source)];
        for (k, v) in [
            ("逐字稿模型", &p.models.stt),
            ("說話者辨識", &p.models.attribution),
            ("摘要模型", &p.models.summary),
        ] {
            if !v.trim().is_empty() && !(k == "摘要模型" && p.summary.is_none()) {
                items.push((k.to_string(), v.clone()));
            }
        }
        let rows: Vec<TableRow> = items
            .into_iter()
            .map(|(k, v)| {
                TableRow::new(vec![
                    cell(vec![Run::new().add_text(k).size(19).color(MUTED)], key_w, Some(ACCENT_TINT)),
                    cell(vec![Run::new().add_text(v).size(19).color(TEXT)], val_w, None),
                ])
            })
            .collect();
        table(rows, vec![key_w, val_w])
    }

    /// 發言者：顏色圓點＋名字、發言時間、佔比、段數。
    fn speaker_table(p: &Project) -> Table {
        let w = [BODY_W - 3 * 1700, 1700, 1700, 1700];
        let head = |t: &str, w: usize| cell(vec![Run::new().add_text(t).bold().size(19).color(ACCENT_DARK)], w, Some(ACCENT_TINT));
        let mut rows = vec![TableRow::new(vec![
            head("說話者", w[0]),
            head("發言時間", w[1]),
            head("佔比", w[2]),
            head("句數", w[3]),
        ])];
        let stats = p.speaker_stats();
        let total: f64 = stats.iter().map(|s| s.1).sum::<f64>().max(1e-9);
        for (id, secs, count) in stats {
            let color = speaker_color(p, Some(&id));
            rows.push(TableRow::new(vec![
                cell(
                    vec![dot(color, 19), Run::new().add_text(p.speaker_name(Some(&id))).size(19).color(TEXT)],
                    w[0],
                    None,
                ),
                cell(vec![Run::new().add_text(duration(secs)).size(19)], w[1], None),
                cell(vec![Run::new().add_text(format!("{:.0}%", secs / total * 100.0)).size(19)], w[2], None),
                cell(vec![Run::new().add_text(count.to_string()).size(19)], w[3], None),
            ]));
        }
        table(rows, w.to_vec())
    }

    /// 摘要的 Markdown 轉 Word：標題、條列、待辦方框、編號、引言、表格、**粗體**（區塊解析見 [`md_blocks`]）。
    fn markdown_into(mut doc: Docx, md: &str, title: &str) -> Docx {
        for block in md_blocks(md, title) {
            match block {
                MdBlock::Table(rows) => {
                    let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                    let w = BODY_W / cols;
                    let rows = rows
                        .iter()
                        .enumerate()
                        .map(|(r, cells)| {
                            TableRow::new(
                                (0..cols)
                                    .map(|c| {
                                        let text = cells.get(c).map(String::as_str).unwrap_or("");
                                        let runs = inline_runs(text, r == 0, if r == 0 { ACCENT_DARK } else { TEXT }, 19);
                                        cell(runs, w, (r == 0).then_some(ACCENT_TINT))
                                    })
                                    .collect(),
                            )
                        })
                        .collect();
                    doc = doc.add_table(table(rows, vec![w; cols]));
                    doc = doc.add_paragraph(Paragraph::new().line_spacing(spacing(0, 60)));
                }
                MdBlock::Heading(level, text) => {
                    let style = format!("Heading{}", (level + 1).min(4));
                    doc = doc.add_paragraph(inline(Paragraph::new().style(&style).keep_next(true), text));
                }
                MdBlock::Item(marker, depth, rest) => {
                    let left = 360 * (depth as i32 + 1);
                    let para = Paragraph::new()
                        .indent(Some(left), Some(SpecialIndentType::Hanging(300)), None, None)
                        .line_spacing(spacing(0, 60));
                    let color = if marker == "•" { ACCENT } else { TEXT };
                    let para = para.add_run(Run::new().add_text(format!("{marker}\t")).color(color));
                    doc = doc.add_paragraph(inline(para, rest));
                }
                MdBlock::Quote(q) => {
                    let mut para = Paragraph::new()
                        .indent(Some(240), None, None, None)
                        .line_spacing(spacing(80, 80));
                    para.property = para.property.set_borders(
                        ParagraphBorders::with_empty().set(
                            ParagraphBorder::new(ParagraphBorderPosition::Left).val(BorderType::Single).size(18).space(8).color(ACCENT),
                        ),
                    );
                    doc = doc.add_paragraph(inline_colored(para, q, MUTED));
                }
                MdBlock::Para(t) => doc = doc.add_paragraph(inline(Paragraph::new().line_spacing(spacing(0, 100)), t)),
            }
        }
        doc
    }

    /// `**粗體**` 切成粗體 run，其他 Markdown 記號拿掉。
    fn inline_runs(text: &str, all_bold: bool, color: &str, size: usize) -> Vec<Run> {
        inline_parts(text)
            .into_iter()
            .map(|(part, bold)| {
                let mut run = Run::new().add_text(part).color(color).size(size);
                if all_bold || bold {
                    run = run.bold();
                }
                run
            })
            .collect()
    }

    fn inline(para: Paragraph, text: &str) -> Paragraph {
        inline_colored(para, text, TEXT)
    }

    fn inline_colored(mut para: Paragraph, text: &str, color: &str) -> Paragraph {
        for run in inline_runs(text, false, color, 22) {
            para = para.add_run(run);
        }
        para
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{JobOptions, Segment, Summary, Timing};

    fn project() -> Project {
        let mut p = Project::new("週會.mp3".into(), JobOptions::default());
        p.duration = 10.0;
        p.add_speaker(Some("王經理".into()), String::new());
        p.segments = vec![
            Segment {
                id: 1,
                start: 0.0,
                end: 2.0,
                timing: Timing::Model,
                speaker: Some("S1".into()),
                text: "開始。".into(),
                chunk: 0,
                edited: false,
            },
            Segment {
                id: 2,
                start: 2.0,
                end: 4.5,
                timing: Timing::Model,
                speaker: Some("S1".into()),
                text: "說 \"預算\"。".into(),
                chunk: 0,
                edited: false,
            },
        ];
        p.summary = Some(Summary {
            markdown: "# 重點\n- **決議**：通過\n\n| 項目 | 負責 |\n|---|---|\n| 預算 | 王 |".into(),
            model: "m".into(),
            created_at: chrono::Local::now(),
            instructions: String::new(),
        });
        p
    }

    fn opts() -> ExportOptions {
        ExportOptions::default()
    }

    fn text(ex: &dyn Exporter, p: &Project, o: &ExportOptions) -> String {
        String::from_utf8(ex.render(p, o).unwrap()).unwrap()
    }

    #[test]
    fn subtitles() {
        let p = project();
        let srt = text(&SrtExporter, &p, &opts());
        assert!(srt.starts_with("1\n00:00:00,000 --> 00:00:02,000\n王經理：開始。"));
        let vtt = text(&VttExporter, &p, &opts());
        assert!(vtt.contains("<v 王經理>開始。"));
        let no_names = ExportOptions {
            speaker_names: false,
            ..opts()
        };
        assert!(text(&SrtExporter, &p, &no_names).contains("00:00:02,000\n開始。"));
    }

    #[test]
    fn csv_quotes() {
        let csv = text(&CsvExporter, &project(), &opts());
        assert!(csv.contains("\"說 \"\"預算\"\"。\""));
    }

    #[test]
    fn markdown_merges_turns_and_demotes() {
        let md = text(&MarkdownExporter, &project(), &opts());
        assert!(md.contains("### 重點"));
        assert!(md.contains("`00:00:00` **王經理**：開始。說 \"預算\"。"));
        assert!(md.contains("- 長度："));
    }

    #[test]
    fn transcript_style_options() {
        let p = project();
        let o = ExportOptions {
            timestamps: false,
            speaker_names: false,
            merge_turns: false,
            header: false,
            ..opts()
        };
        let md = text(&MarkdownExporter, &p, &o);
        assert!(md.contains("\n開始。\n\n說 \"預算\"。\n"), "{md}");
        assert!(!md.contains("長度："));
        assert!(!md.contains("## 發言者"));
        let txt = text(&TextExporter, &p, &o);
        assert!(txt.contains("開始。\r\n說 \"預算\"。\r\n"), "{txt}");
    }

    #[test]
    fn content_choices() {
        let p = project();
        let only = |content| ExportOptions { content, ..opts() };
        let s = text(&MarkdownExporter, &p, &only(Content::SummaryOnly));
        assert!(s.starts_with("# 週會 摘要"));
        assert!(s.contains("### 重點") && !s.contains("## 逐字稿"));
        let t = text(&MarkdownExporter, &p, &only(Content::TranscriptOnly));
        assert!(t.starts_with("# 週會 逐字稿"));
        assert!(!t.contains("會議摘要") && t.contains("## 逐字稿"));
        // 純文字也放摘要，Markdown 記號拿掉
        let txt = text(&TextExporter, &p, &only(Content::Both));
        assert!(txt.contains("【會議摘要】") && txt.contains("決議：通過") && !txt.contains("**"));
        assert!(!txt.contains("|---"));
    }

    #[test]
    fn docx_is_a_zip() {
        let p = project();
        for content in Content::ALL {
            let bytes = DocxExporter.render(&p, &ExportOptions { content, ..opts() }).unwrap();
            assert_eq!(&bytes[..2], b"PK");
        }
    }

    /// 2026-10-31「docx也要修 我希望把排版...等變得更好」：頁碼、待辦方框、編號、標題不重複；
    /// 同日「docx更新成不放逐字稿」：內容選項設成摘要＋逐字稿也只放摘要。
    #[test]
    fn docx_layout_has_page_numbers_checkboxes_and_no_transcript() {
        use std::io::Read;
        let mut p = project();
        p.title = "重點".into();
        p.summary.as_mut().unwrap().markdown = "# 重點
- [ ] 寄企劃書
- [x] 定好檢討會
1. 先報價
> 備註".into();
        let bytes = DocxExporter.render(&p, &opts()).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut read = |name: &str| {
            let mut s = String::new();
            zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        let doc = read("word/document.xml");
        assert!(doc.contains("☐") && doc.contains("☑"), "待辦用方框");
        assert!(doc.contains(">1.<") || doc.contains(">1.	<") || doc.contains("1.	"), "編號保留");
        assert!(!doc.contains("開始。") && !doc.contains("逐字稿<"), "Word 不放逐字稿");
        assert!(doc.contains(">發言者<"), "發言者統計留著");
        assert_eq!(doc.matches(">重點 摘要<").count(), 1, "標題區是「標題 摘要」，摘要第一行跟會議名稱一樣的 # 標題略過");
        assert!(doc.contains("w:w=\"") && doc.contains("w:type=\"dxa\""), "表格固定寬度");
        let footer: String = (1..=3)
            .filter_map(|i| zip.by_name(&format!("word/footer{i}.xml")).ok().map(|mut f| {
                let mut s = String::new();
                f.read_to_string(&mut s).unwrap();
                s
            }))
            .collect();
        assert!(footer.contains("PAGE") && footer.contains("NUMPAGES"), "頁尾有頁碼");
    }

    /// 「輸出檔案也加入PDF並預設勾選」：預設格式含 PDF；Word、PDF 沒有摘要就不產，只選它們時要報錯。
    #[test]
    fn default_formats_include_pdf_and_summary_only_formats_skip_without_summary() {
        if !pdf::font_available() {
            eprintln!("這台沒有中文字型，略過");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project();
        assert_eq!(export_into(dir.path(), &p, opts()).unwrap(), ["週會.md", "週會.docx", "週會.pdf", "週會.srt"]);
        p.summary = None;
        let names = export_into(dir.path(), &p, opts()).unwrap();
        assert_eq!(names, ["週會 (2).md", "週會 (2).srt"], "沒摘要：Word、PDF 不產");
        let only = ExportOptions { formats: vec!["docx".into(), "pdf".into()], ..opts() };
        let e = export_into(dir.path(), &p, only).unwrap_err().to_string();
        assert!(e.contains("Word、PDF 只放摘要"), "{e}");
    }

    #[test]
    fn pdf_has_pages_fonts_and_no_transcript() {
        if !pdf::font_available() {
            eprintln!("這台沒有中文字型，略過");
            return;
        }
        let mut p = project();
        p.summary.as_mut().unwrap().markdown =
            format!("# 重點
- [ ] 寄企劃書
- [x] 定好檢討會
1. 先報價
> 備註

{}", "很長的段落，".repeat(400));
        let bytes = PdfExporter.render(&p, &opts()).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() < 2_000_000, "字型有子集化（不是整個 20 MB 嵌進去）：{}", bytes.len());
        // 留一份給人看（target/pdf-shots/，同 UI 截圖）；頁數、文字由 tests 外用 PyMuPDF 驗
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/pdf-shots");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("摘要.pdf"), &bytes).unwrap();
        let mut docx = std::io::Cursor::new(Vec::new());
        std::io::Write::write_all(&mut docx, &DocxExporter.render(&p, &opts()).unwrap()).unwrap();
        std::fs::write(out.join("摘要.docx"), docx.into_inner()).unwrap();
    }

    #[test]
    fn pdf_line_breaking_keeps_words_and_punctuation() {
        if !pdf::font_available() {
            return;
        }
        // 11 pt 的中文字約 11 pt 寬：50 pt 放得下 4 個字
        let lines = pdf::line_breaks("一二三四，五六七", 50.0);
        assert!(lines.iter().all(|l| !l.starts_with('，')), "句讀不放行首：{lines:?}");
        let lines = pdf::line_breaks("請看 OpenRouter 的帳單", 70.0);
        assert!(lines.iter().any(|l| l.contains("OpenRouter")), "英文字不拆開：{lines:?}");
        assert_eq!(lines.concat().replace(' ', ""), "請看OpenRouter的帳單");
    }

    #[test]
    fn filename_template_and_subfolder() {
        let mut p = project();
        p.source_file = PathBuf::from(r"C:\錄音\週會.mp3");
        p.title = "週會:預算/人力".into();
        let o = ExportOptions {
            filename: "{日期} {標題}".into(),
            subfolder: false,
            ..opts()
        };
        let date = p.created_at.format("%Y-%m-%d").to_string();
        assert_eq!(o.file_stem(&p), format!("{date} 週會_預算_人力"));
        assert_eq!(o.target_dir(&p), PathBuf::from(r"C:\錄音"));
        let o = ExportOptions {
            dir: r"D:\會議記錄".into(),
            subfolder: true,
            ..o
        };
        assert_eq!(o.target_dir(&p), PathBuf::from(format!(r"D:\會議記錄\{date} 週會_預算_人力")));
        let blank = ExportOptions {
            filename: "  ".into(),
            ..opts()
        };
        assert_eq!(blank.file_stem(&p), "週會_預算_人力");
    }

    fn export_into(dir: &Path, p: &Project, o: ExportOptions) -> Result<Vec<String>> {
        let o = ExportOptions {
            dir: dir.display().to_string(),
            ..o
        };
        Ok(export_project(p, &o)?
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect())
    }

    #[test]
    fn export_avoids_or_allows_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = project();
        let txt = ExportOptions {
            formats: vec!["txt".into()],
            ..opts()
        };
        assert_eq!(export_into(dir.path(), &p, txt.clone()).unwrap(), ["週會.txt"]);
        assert_eq!(export_into(dir.path(), &p, txt.clone()).unwrap(), ["週會 (2).txt"]);
        let over = ExportOptions { overwrite: true, ..txt };
        assert_eq!(export_into(dir.path(), &p, over).unwrap(), ["週會.txt"]);
    }

    #[test]
    fn separate_files_and_missing_summary() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = project();
        let sep = ExportOptions {
            formats: vec!["srt".into(), "md".into(), "bogus".into()],
            content: Content::Separate,
            ..opts()
        };
        // 依 registry 順序；字幕不分檔
        assert_eq!(
            export_into(dir.path(), &p, sep.clone()).unwrap(),
            ["週會 摘要.md", "週會 逐字稿.md", "週會.srt"]
        );
        p.summary = None;
        let only_summary = ExportOptions {
            formats: vec!["md".into(), "docx".into()],
            content: Content::SummaryOnly,
            ..opts()
        };
        assert!(export_into(dir.path(), &p, only_summary).is_err(), "沒摘要又只要摘要要報錯");
        let names = export_into(dir.path(), &p, sep).unwrap();
        assert_eq!(names, ["週會 逐字稿 (2).md", "週會 (2).srt"]);
        let none = ExportOptions {
            formats: vec![],
            ..opts()
        };
        assert!(export_into(dir.path(), &p, none).is_err());
    }
}
