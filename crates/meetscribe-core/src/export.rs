//! 匯出。每種格式一個 [`Exporter`]，加新格式只要在 [`registry`] 多登記一個。

use crate::model::Project;
use crate::textutil::{fmt_duration_zh, fmt_hms, fmt_subtitle, safe_filename};
use crate::{Error, Result};
use std::path::{Path, PathBuf};

pub trait Exporter: Send + Sync {
    /// 設定檔裡記的代號。
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn extension(&self) -> &'static str;
    fn render(&self, project: &Project) -> Result<Vec<u8>>;
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

/// 把選定的格式寫到資料夾，檔名「<會議名>.<副檔名>」，已存在就加 (2)、(3)。
pub fn export_all(project: &Project, ids: &[String], dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let base = safe_filename(&project.title);
    let mut written = Vec::new();
    for ex in registry().into_iter().filter(|e| ids.iter().any(|i| i == e.id())) {
        let bytes = ex.render(project)?;
        let path = unique_path(dir, &base, ex.extension());
        std::fs::write(&path, bytes)?;
        written.push(path);
    }
    if written.is_empty() {
        return Err(Error::Other("沒有選任何匯出格式".into()));
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

/// 逐字稿的一列：時間、說話者名稱、文字。
fn rows(project: &Project) -> Vec<(f64, f64, String, &str)> {
    project
        .segments
        .iter()
        .map(|s| {
            let name = if s.speaker.is_some() {
                project.speaker_name(s.speaker.as_deref())
            } else {
                String::new()
            };
            (s.start, s.end, name, s.text.as_str())
        })
        .collect()
}

/// 同一人連續發言併成一段（閱讀用格式）。
fn turns(project: &Project) -> Vec<(f64, String, String)> {
    let mut out: Vec<(f64, String, String)> = Vec::new();
    let mut last: Option<Option<String>> = None;
    for s in &project.segments {
        if last.as_ref() == Some(&s.speaker)
            && let Some(t) = out.last_mut()
        {
            if crate::model::needs_space(&t.2, &s.text) {
                t.2.push(' ');
            }
            t.2.push_str(&s.text);
            continue;
        }
        let name = if s.speaker.is_some() {
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

pub struct MarkdownExporter;
impl Exporter for MarkdownExporter {
    fn id(&self) -> &'static str {
        "md"
    }
    fn label(&self) -> &'static str {
        "Markdown（摘要＋逐字稿）"
    }
    fn extension(&self) -> &'static str {
        "md"
    }
    fn render(&self, p: &Project) -> Result<Vec<u8>> {
        let mut s = format!("# {}\n\n", p.title);
        for m in meta_lines(p) {
            s.push_str(&format!("- {m}\n"));
        }
        if let Some(sum) = &p.summary {
            s.push_str("\n## 會議摘要\n\n");
            s.push_str(&demote_headings(&sum.markdown, 2));
            s.push('\n');
        }
        if !p.speakers.is_empty() {
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
        for (start, name, text) in turns(p) {
            if name.is_empty() {
                s.push_str(&format!("`{}` {}\n\n", fmt_hms(start), text));
            } else {
                s.push_str(&format!("`{}` **{}**：{}\n\n", fmt_hms(start), name, text));
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

pub struct TextExporter;
impl Exporter for TextExporter {
    fn id(&self) -> &'static str {
        "txt"
    }
    fn label(&self) -> &'static str {
        "純文字逐字稿"
    }
    fn extension(&self) -> &'static str {
        "txt"
    }
    fn render(&self, p: &Project) -> Result<Vec<u8>> {
        let mut s = String::from("\u{feff}");
        for (start, name, text) in turns(p) {
            if name.is_empty() {
                s.push_str(&format!("[{}] {}\r\n\r\n", fmt_hms(start), text));
            } else {
                s.push_str(&format!("[{}] {}：{}\r\n\r\n", fmt_hms(start), name, text));
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
    fn render(&self, p: &Project) -> Result<Vec<u8>> {
        let mut s = String::new();
        for (i, (start, end, name, text)) in rows(p).into_iter().enumerate() {
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
        "WebVTT 字幕（含說話者）"
    }
    fn extension(&self) -> &'static str {
        "vtt"
    }
    fn render(&self, p: &Project) -> Result<Vec<u8>> {
        let mut s = String::from("WEBVTT\n\n");
        for (start, end, name, text) in rows(p) {
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
    fn render(&self, p: &Project) -> Result<Vec<u8>> {
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut s = String::from("\u{feff}開始,結束,說話者,內容\r\n");
        for (start, end, name, text) in rows(p) {
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
    fn render(&self, p: &Project) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(p)?)
    }
}

pub struct DocxExporter;
impl Exporter for DocxExporter {
    fn id(&self) -> &'static str {
        "docx"
    }
    fn label(&self) -> &'static str {
        "Word 文件（摘要＋逐字稿）"
    }
    fn extension(&self) -> &'static str {
        "docx"
    }
    fn render(&self, p: &Project) -> Result<Vec<u8>> {
        docx::render(p)
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

    pub fn render(p: &Project) -> Result<Vec<u8>> {
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
                .add_run(Run::new().add_text(&p.title)),
        );
        for m in meta_lines(p) {
            doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_text(m).size(18).color("666666")));
        }
        if let Some(sum) = &p.summary {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .style("Heading2")
                    .add_run(Run::new().add_text("會議摘要")),
            );
            doc = markdown_into(doc, &sum.markdown);
        }
        if !p.speakers.is_empty() {
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
        for (start, name, text) in turns(p) {
            let mut para = Paragraph::new().add_run(
                Run::new()
                    .add_text(format!("[{}] ", fmt_hms(start)))
                    .color("888888")
                    .size(18),
            );
            if !name.is_empty() {
                para = para.add_run(Run::new().add_text(format!("{name}：")).bold());
            }
            doc = doc.add_paragraph(para.add_run(Run::new().add_text(text)));
        }
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

    #[test]
    fn subtitles() {
        let p = project();
        let srt = String::from_utf8(SrtExporter.render(&p).unwrap()).unwrap();
        assert!(srt.starts_with("1\n00:00:00,000 --> 00:00:02,000\n王經理：開始。"));
        let vtt = String::from_utf8(VttExporter.render(&p).unwrap()).unwrap();
        assert!(vtt.contains("<v 王經理>開始。"));
    }

    #[test]
    fn csv_quotes() {
        let csv = String::from_utf8(CsvExporter.render(&project()).unwrap()).unwrap();
        assert!(csv.contains("\"說 \"\"預算\"\"。\""));
    }

    #[test]
    fn markdown_merges_turns_and_demotes() {
        let md = String::from_utf8(MarkdownExporter.render(&project()).unwrap()).unwrap();
        assert!(md.contains("### 重點"));
        assert!(md.contains("**王經理**：開始。說 \"預算\"。"));
    }

    #[test]
    fn docx_is_a_zip() {
        let bytes = DocxExporter.render(&project()).unwrap();
        assert_eq!(&bytes[..2], b"PK");
    }

    #[test]
    fn export_all_avoids_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = project();
        let ids = vec!["txt".to_string()];
        let a = export_all(&p, &ids, dir.path()).unwrap();
        let b = export_all(&p, &ids, dir.path()).unwrap();
        assert_ne!(a[0], b[0]);
        assert!(b[0].to_string_lossy().ends_with("週會 (2).txt"));
    }
}
