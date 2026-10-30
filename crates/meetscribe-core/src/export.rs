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
    fn render(&self, project: &Project, opts: &ExportOptions) -> Result<Vec<u8>>;
}

/// 摘要與逐字稿怎麼放（只影響 Word、Markdown、純文字；字幕、CSV、JSON 固定是逐字稿／完整資料）。
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
    /// 每場會議另建一個子資料夾（名稱同檔名）。
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
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            auto: true,
            formats: vec!["docx".into(), "md".into(), "srt".into()],
            dir: String::new(),
            subfolder: false,
            filename: FILENAME_PRESETS[0].into(),
            overwrite: false,
            content: Content::Both,
            timestamps: true,
            speaker_names: true,
            merge_turns: true,
            header: true,
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
    if opts.content == Content::SummaryOnly
        && project.summary.is_none()
        && registry().iter().all(|e| !ids.contains(&e.id()) || e.has_summary())
    {
        return Err(Error::Other("這場會議還沒有摘要，「只要摘要」沒有東西可匯出".into()));
    }
    let dir = opts.target_dir(project);
    std::fs::create_dir_all(&dir)?;
    let base = opts.file_stem(project);
    let mut written = Vec::new();
    for ex in registry().into_iter().filter(|e| ids.contains(&e.id())) {
        // (檔名後綴, 這個檔的內容選項)
        let parts: Vec<(&str, Content)> = match (ex.has_summary(), opts.content) {
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
    let mut v = vec![
        format!("原始檔案：{}", project.source_file.display()),
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
        "Word 文件"
    }
    fn extension(&self) -> &'static str {
        "docx"
    }
    fn has_summary(&self) -> bool {
        true
    }
    fn render(&self, p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        docx::render(p, o)
    }
}

mod docx {
    use super::*;
    use docx_rs::*;

    const FONT_EA: &str = "微軟正黑體";
    const FONT_LATIN: &str = "Calibri";

    fn heading_style(id: &str, name: &str, size: usize) -> Style {
        Style::new(id, StyleType::Paragraph).name(name).size(size).bold()
    }

    pub fn render(p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
        let mut doc = Docx::new()
            .default_fonts(RunFonts::new().east_asia(FONT_EA).ascii(FONT_LATIN).hi_ansi(FONT_LATIN))
            .default_size(22)
            .add_style(heading_style("Heading1", "heading 1", 36))
            .add_style(heading_style("Heading2", "heading 2", 30))
            .add_style(heading_style("Heading3", "heading 3", 26))
            .add_style(heading_style("Heading4", "heading 4", 24));
        doc = doc.add_paragraph(
            Paragraph::new()
                .style("Heading1")
                .add_run(Run::new().add_text(doc_title(p, o))),
        );
        if o.header {
            for m in meta_lines(p) {
                doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_text(m).size(18).color("666666")));
            }
        }
        if o.content.summary()
            && let Some(sum) = &p.summary
        {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .style("Heading2")
                    .add_run(Run::new().add_text("會議摘要")),
            );
            doc = markdown_into(doc, &sum.markdown);
        }
        if !o.content.transcript() {
            return pack(doc);
        }
        if o.header && !p.speakers.is_empty() {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .style("Heading2")
                    .add_run(Run::new().add_text("發言者")),
            );
            let mut table_rows = vec![row(&["說話者", "發言時間", "句數"], true)];
            for (id, secs, count) in p.speaker_stats() {
                table_rows.push(row(
                    &[&p.speaker_name(Some(&id)), &fmt_duration_zh(secs), &count.to_string()],
                    false,
                ));
            }
            doc = doc.add_table(Table::new(table_rows));
        }
        doc = doc.add_paragraph(
            Paragraph::new()
                .style("Heading2")
                .add_run(Run::new().add_text("逐字稿")),
        );
        for (start, name, text) in turns(p, o) {
            let mut para = Paragraph::new();
            if o.timestamps {
                para = para.add_run(
                    Run::new()
                        .add_text(format!("[{}] ", fmt_hms(start)))
                        .color("888888")
                        .size(18),
                );
            }
            if !name.is_empty() {
                para = para.add_run(Run::new().add_text(format!("{name}：")).bold());
            }
            doc = doc.add_paragraph(para.add_run(Run::new().add_text(text)));
        }
        pack(doc)
    }

    fn pack(doc: Docx) -> Result<Vec<u8>> {
        let mut buf = std::io::Cursor::new(Vec::new());
        doc.build()
            .pack(&mut buf)
            .map_err(|e| Error::Other(format!("產生 Word 檔失敗：{e}")))?;
        Ok(buf.into_inner())
    }

    fn row(cells: &[&str], header: bool) -> TableRow {
        TableRow::new(
            cells
                .iter()
                .map(|c| {
                    let mut run = Run::new().add_text(*c);
                    if header {
                        run = run.bold();
                    }
                    let mut cell = TableCell::new().add_paragraph(Paragraph::new().add_run(run));
                    if header {
                        cell = cell.shading(Shading::new().fill("E8EEF4"));
                    }
                    cell
                })
                .collect(),
        )
    }

    /// 摘要的 Markdown 轉 Word：標題、條列、表格、**粗體**。
    fn markdown_into(mut doc: Docx, md: &str) -> Docx {
        let lines: Vec<&str> = md.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].trim_end();
            let trimmed = line.trim_start();
            if trimmed.starts_with('|') {
                let mut table_rows = Vec::new();
                let mut first = true;
                while i < lines.len() && lines[i].trim_start().starts_with('|') {
                    let cells: Vec<String> = lines[i]
                        .trim()
                        .trim_matches('|')
                        .split('|')
                        .map(|c| c.trim().replace("**", ""))
                        .collect();
                    let is_sep = cells
                        .iter()
                        .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
                    if !is_sep {
                        let refs: Vec<&str> = cells.iter().map(String::as_str).collect();
                        table_rows.push(row(&refs, first));
                        first = false;
                    }
                    i += 1;
                }
                if !table_rows.is_empty() {
                    doc = doc.add_table(Table::new(table_rows));
                }
                continue;
            }
            if let Some(level) = heading_level(trimmed) {
                let text = trimmed.trim_start_matches('#').trim();
                let style = format!("Heading{}", (level + 2).min(4));
                doc = doc.add_paragraph(inline(Paragraph::new().style(&style), text));
            } else if let Some(rest) = bullet(trimmed) {
                let depth = (line.len() - trimmed.len()) / 2;
                let para = Paragraph::new().indent(
                    Some(360 * (depth as i32 + 1)),
                    Some(SpecialIndentType::Hanging(240)),
                    None,
                    None,
                );
                doc = doc.add_paragraph(inline(para, &format!("• {rest}")));
            } else if trimmed.is_empty() || trimmed.chars().all(|c| matches!(c, '-' | '*' | '_')) {
                // 空行與分隔線略過
            } else {
                doc = doc.add_paragraph(inline(Paragraph::new(), trimmed.trim_start_matches('>').trim()));
            }
            i += 1;
        }
        doc
    }

    fn heading_level(s: &str) -> Option<usize> {
        let n = s.chars().take_while(|c| *c == '#').count();
        (n > 0 && s.chars().nth(n) == Some(' ')).then_some(n)
    }

    fn bullet(s: &str) -> Option<&str> {
        for p in ["- [ ] ", "- [x] ", "- ", "* ", "+ "] {
            if let Some(r) = s.strip_prefix(p) {
                return Some(r);
            }
        }
        // 1. 2. 這類編號保留原編號
        let digits = s.chars().take_while(char::is_ascii_digit).count();
        (digits > 0 && s[digits..].starts_with(". ")).then_some(s)
    }

    /// `**粗體**` 切成粗體 run，其他 Markdown 記號拿掉。
    fn inline(mut para: Paragraph, text: &str) -> Paragraph {
        for (i, part) in text.split("**").enumerate() {
            if part.is_empty() {
                continue;
            }
            let clean = part.replace('`', "");
            let mut run = Run::new().add_text(clean);
            if i % 2 == 1 {
                run = run.bold();
            }
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

    #[test]
    fn filename_template_and_subfolder() {
        let mut p = project();
        p.source_file = PathBuf::from(r"C:\錄音\週會.mp3");
        p.title = "週會:預算/人力".into();
        let o = ExportOptions {
            filename: "{日期} {標題}".into(),
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
