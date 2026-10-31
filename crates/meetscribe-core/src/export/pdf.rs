//! PDF 會議摘要（2026-10-31 使用者：「PDF是從Docx轉成PDF」）。
//!
//! 手機上沒有 Word／LibreOffice 能轉檔，所以照 [`super::docx`] 的版面直接畫：同樣的 A4 與邊界、頁首會議名稱、頁尾頁碼、
//! 標題區、會議資訊表、摘要（同一份 [`md_blocks`] 解析）、發言者表，色票與字級也一樣。
//! 字型用系統的中文字型（電腦：微軟正黑體，同 Word 檔；Android：Noto Sans CJK），krilla 只嵌入用到的字。

use super::docx::{ACCENT, ACCENT_DARK, ACCENT_TINT, FONT_EA, MUTED, RULE, SPEAKER_COLORS, TEXT};
use super::{MdBlock, inline_parts, md_blocks};
use crate::model::Project;
use crate::textutil::fmt_duration_zh;
use crate::{Error, Result};
use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::surface::Surface;
use krilla::text::{Font, TextDirection};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use super::ExportOptions;

/// A4（pt），邊界 2.2 cm、頁首頁尾距紙邊 620 twip，同 Word 檔。
const PAGE_W: f32 = 595.3;
const PAGE_H: f32 = 841.9;
const MARGIN: f32 = 62.35;
const HEADER_Y: f32 = 31.0;
const BODY_W: f32 = PAGE_W - 2.0 * MARGIN;
/// 行高倍數：Word 的 1.3 倍行距 × 正黑體本身的行距。
const LINE: f32 = 1.62;

pub fn render(p: &Project, o: &ExportOptions) -> Result<Vec<u8>> {
    let fonts = fonts().as_ref().map_err(|e| Error::Other(e.clone()))?;
    let mut l = Layout::new(fonts);
    let title = super::doc_title(p, o);

    // 標題區
    l.para(&[span("會議摘要", 10.0, ACCENT, true)], 0.0, 0.0, 2.0);
    l.para(&[span(&title, 24.0, TEXT, true)], 0.0, 0.0, 3.0);
    let mut sub = vec![p.created_at.format("%Y-%m-%d %H:%M").to_string()];
    if p.duration > 0.0 {
        sub.push(fmt_duration_zh(p.duration).replace(' ', ""));
    }
    if !p.speakers.is_empty() {
        sub.push(format!("{} 位發言者", p.speakers.len()));
    }
    l.para(&[span(&sub.join("　·　"), 10.0, MUTED, false)], 0.0, 0.0, 4.0);
    l.rule(ACCENT, 1.5);
    l.y += 12.0;
    if o.header {
        info_table(&mut l, p);
    }
    if let Some(sum) = &p.summary {
        l.heading1("會議摘要");
        markdown(&mut l, &sum.markdown, &p.title);
    }
    if o.header && !p.speakers.is_empty() {
        l.heading1("發言者");
        speaker_table(&mut l, p);
    }
    l.finish(&p.title)
}

// ---------- 版面 ----------

#[derive(Clone)]
struct Span {
    text: String,
    size: f32,
    color: &'static str,
    bold: bool,
}

fn span(text: &str, size: f32, color: &'static str, bold: bool) -> Span {
    Span { text: text.to_string(), size, color, bold }
}

/// `**粗體**` 的一段文字轉成 [`Span`]。
fn rich(text: &str, size: f32, color: &'static str, all_bold: bool) -> Vec<Span> {
    inline_parts(text)
        .into_iter()
        .map(|(t, b)| Span { text: t, size, color, bold: all_bold || b })
        .collect()
}

/// 畫在某一頁上的東西（排完版、知道總頁數後才真的畫，頁尾要「共 N 頁」）。
enum Op {
    Text { x: f32, baseline: f32, size: f32, color: &'static str, face: usize, synth_bold: bool, text: String },
    Rect { x: f32, y: f32, w: f32, h: f32, color: &'static str },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: &'static str },
}

/// 一行裡同字型、同樣式的一段。
struct Piece {
    text: String,
    size: f32,
    color: &'static str,
    face: usize,
    synth_bold: bool,
    width: f32,
}

struct TextLine {
    pieces: Vec<Piece>,
    size: f32,
}

impl TextLine {
    fn height(&self) -> f32 {
        self.size * LINE
    }
}

struct Layout<'a> {
    fonts: &'a Fonts,
    pages: Vec<Vec<Op>>,
    y: f32,
}

impl<'a> Layout<'a> {
    fn new(fonts: &'a Fonts) -> Self {
        Self { fonts, pages: vec![Vec::new()], y: MARGIN }
    }

    fn ops(&mut self) -> &mut Vec<Op> {
        self.pages.last_mut().expect("page")
    }

    fn at_top(&self) -> bool {
        self.y <= MARGIN + 0.01
    }

    /// 剩下的空間放不下 `h` 就換頁。
    fn need(&mut self, h: f32) {
        if self.y + h > PAGE_H - MARGIN && !self.at_top() {
            self.pages.push(Vec::new());
            self.y = MARGIN;
        }
    }

    /// 段前距（在頁首時不加，同 Word）。
    fn space(&mut self, pt: f32) {
        if !self.at_top() {
            self.y += pt;
        }
    }

    /// 斷行：中日韓字之間都可以斷，英數字整個字一起；句讀不放行首（懸在行尾）。
    fn wrap(&self, spans: &[Span], width: f32) -> Vec<TextLine> {
        // (字, 樣式索引)；「字」是一個中文字、一個空白，或一串連續的英數
        let mut atoms: Vec<(String, usize)> = Vec::new();
        for (si, s) in spans.iter().enumerate() {
            let mut word = String::new();
            for c in s.text.chars() {
                if c == '\n' || c == '\r' {
                    continue;
                }
                if c.is_whitespace() || is_cjk(c) {
                    if !word.is_empty() {
                        atoms.push((std::mem::take(&mut word), si));
                    }
                    atoms.push((c.to_string(), si));
                } else {
                    word.push(c);
                }
            }
            if !word.is_empty() {
                atoms.push((word, si));
            }
        }
        let mut lines: Vec<Vec<(String, usize)>> = vec![Vec::new()];
        let mut cur = 0.0;
        for (text, si) in atoms {
            let s = &spans[si];
            let space = text.chars().all(char::is_whitespace);
            let w = self.fonts.width(&text, s.size, s.bold);
            let line = lines.last_mut().expect("line");
            if line.is_empty() && space {
                continue;
            }
            if cur + w <= width || line.is_empty() || is_no_break_start(&text) {
                if w > width && line.is_empty() && !space {
                    // 一個字比整行還寬（很長的網址）：逐字拆
                    for c in text.chars() {
                        let cw = self.fonts.width(&c.to_string(), s.size, s.bold);
                        let line = lines.last_mut().expect("line");
                        if cur + cw > width && !line.is_empty() {
                            lines.push(Vec::new());
                            cur = 0.0;
                        }
                        lines.last_mut().expect("line").push((c.to_string(), si));
                        cur += cw;
                    }
                } else {
                    line.push((text, si));
                    cur += w;
                }
                continue;
            }
            while line.last().is_some_and(|(t, _)| t.chars().all(char::is_whitespace)) {
                line.pop();
            }
            lines.push(Vec::new());
            cur = 0.0;
            if !space {
                lines.last_mut().expect("line").push((text, si));
                cur = w;
            }
        }
        lines
            .into_iter()
            .filter(|l| !l.is_empty())
            .map(|atoms| {
                let size = atoms.iter().map(|(_, si)| spans[*si].size).fold(0.0, f32::max);
                let mut pieces: Vec<Piece> = Vec::new();
                for (text, si) in atoms {
                    let s = &spans[si];
                    for c in text.chars() {
                        let g = self.fonts.glyph(c, s.bold);
                        let w = g.advance * s.size;
                        match pieces.last_mut() {
                            Some(pc) if pc.face == g.face && pc.synth_bold == g.synth_bold && pc.size == s.size && pc.color == s.color => {
                                pc.text.push(c);
                                pc.width += w;
                            }
                            _ => pieces.push(Piece {
                                text: c.to_string(),
                                size: s.size,
                                color: s.color,
                                face: g.face,
                                synth_bold: g.synth_bold,
                                width: w,
                            }),
                        }
                    }
                }
                TextLine { pieces, size }
            })
            .collect()
    }

    fn draw_line(&mut self, line: TextLine, x: f32, top: f32) {
        let baseline = top + (line.height() - line.size) / 2.0 + line.size * 0.88;
        let mut x = x;
        for pc in line.pieces {
            let w = pc.width;
            self.ops().push(Op::Text {
                x,
                baseline,
                size: pc.size,
                color: pc.color,
                face: pc.face,
                synth_bold: pc.synth_bold,
                text: pc.text,
            });
            x += w;
        }
    }

    /// 一段文字：左縮排、段前、段後（pt）。
    fn para(&mut self, spans: &[Span], indent: f32, before: f32, after: f32) {
        self.space(before);
        for line in self.wrap(spans, BODY_W - indent) {
            let h = line.height();
            self.need(h);
            let top = self.y;
            self.draw_line(line, MARGIN + indent, top);
            self.y += h;
        }
        self.y += after;
    }

    /// 段落下緣的橫線（Word 的段落下框線）。
    fn rule(&mut self, color: &'static str, width: f32) {
        let y = self.y + width / 2.0;
        self.ops().push(Op::Line { x1: MARGIN, y1: y, x2: PAGE_W - MARGIN, y2: y, width, color });
        self.y += width;
    }

    /// 章節標題（Heading1）：主色、細底線；跟下一段放同一頁。
    fn heading1(&mut self, text: &str) {
        self.space(18.0);
        self.need(16.0 * LINE + 4.0 + 8.0 + 3.0 * 11.0 * LINE);
        self.para(&[span(text, 16.0, ACCENT_DARK, true)], 0.0, 0.0, 2.0);
        self.rule(RULE, 0.75);
        self.y += 8.0;
    }

    /// 表格：固定欄寬、淡灰框線、儲存格上下 2.5 pt 左右 6 pt 內距；一列放不下就整列換頁。
    fn table(&mut self, rows: Vec<Vec<(Vec<Span>, Option<&'static str>)>>, widths: &[f32]) {
        const PAD_X: f32 = 6.0;
        const PAD_Y: f32 = 2.5;
        const BORDER: f32 = 0.5;
        let total: f32 = widths.iter().sum();
        for row in rows {
            let cells: Vec<(Vec<TextLine>, Option<&'static str>)> = row
                .into_iter()
                .zip(widths)
                .map(|((spans, fill), w)| (self.wrap(&spans, w - 2.0 * PAD_X), fill))
                .collect();
            let h = cells
                .iter()
                .map(|(lines, _)| lines.iter().map(TextLine::height).sum::<f32>())
                .fold(0.0, f32::max)
                + 2.0 * PAD_Y;
            self.need(h);
            let top = self.y;
            let mut x = MARGIN;
            for ((lines, fill), w) in cells.into_iter().zip(widths) {
                if let Some(color) = fill {
                    self.ops().push(Op::Rect { x, y: top, w: *w, h, color });
                }
                // 垂直置中
                let content: f32 = lines.iter().map(TextLine::height).sum();
                let mut y = top + (h - content) / 2.0;
                for line in lines {
                    let lh = line.height();
                    self.draw_line(line, x + PAD_X, y);
                    y += lh;
                }
                x += w;
            }
            let ops = self.ops();
            for y in [top, top + h] {
                ops.push(Op::Line { x1: MARGIN, y1: y, x2: MARGIN + total, y2: y, width: BORDER, color: RULE });
            }
            let mut x = MARGIN;
            for w in std::iter::once(&0.0).chain(widths) {
                x += w;
                ops.push(Op::Line { x1: x, y1: top, x2: x, y2: top + h, width: BORDER, color: RULE });
            }
            self.y += h;
        }
    }

    /// 畫出每一頁，加上頁首（會議名稱）與頁尾（第 N 頁／共 M 頁）。
    fn finish(self, header: &str) -> Result<Vec<u8>> {
        let fonts = self.fonts;
        let total = self.pages.len();
        let mut doc = Document::new();
        doc.set_metadata(
            Metadata::new()
                .title(header.to_string())
                .creator("語音轉錄".to_string())
                .language("zh-TW".to_string()),
        );
        let settings = PageSettings::from_wh(PAGE_W, PAGE_H).ok_or_else(|| Error::Other("PDF 頁面大小錯誤".into()))?;
        for (i, mut ops) in self.pages.into_iter().enumerate() {
            let mut extra = Layout::new(fonts);
            let head = extra.wrap(&[span(header, 8.0, MUTED, false)], BODY_W);
            if let Some(line) = head.into_iter().next() {
                let w: f32 = line.pieces.iter().map(|p| p.width).sum();
                extra.draw_line(line, PAGE_W - MARGIN - w, HEADER_Y);
            }
            let foot = format!("第 {} 頁／共 {} 頁　·　語音轉錄", i + 1, total);
            if let Some(line) = extra.wrap(&[span(&foot, 8.0, MUTED, false)], BODY_W).into_iter().next() {
                let w: f32 = line.pieces.iter().map(|p| p.width).sum();
                let h = line.height();
                extra.draw_line(line, (PAGE_W - w) / 2.0, PAGE_H - HEADER_Y - h);
            }
            ops.extend(extra.pages.into_iter().flatten());

            let mut page = doc.start_page_with(settings.clone());
            let mut surface = page.surface();
            for op in ops {
                draw(&mut surface, fonts, op);
            }
            surface.finish();
            page.finish();
        }
        doc.finish().map_err(|e| Error::Other(format!("產生 PDF 失敗：{e:?}")))
    }
}

fn color(hex: &str) -> rgb::Color {
    let v = u32::from_str_radix(hex, 16).unwrap_or(0);
    rgb::Color::new((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn fill(hex: &str) -> Fill {
    Fill { paint: color(hex).into(), opacity: NormalizedF32::ONE, rule: Default::default() }
}

fn draw(surface: &mut Surface<'_>, fonts: &Fonts, op: Op) {
    match op {
        Op::Text { x, baseline, size, color: c, face, synth_bold, text } => {
            surface.set_fill(Some(fill(c)));
            // 字型沒有粗體（Android 的 Noto Sans CJK 只有一種粗細）：描一圈同色的邊
            surface.set_stroke(synth_bold.then(|| Stroke { paint: color(c).into(), width: size * 0.035, ..Stroke::default() }));
            surface.draw_text(Point::from_xy(x, baseline), fonts.faces[face].font.clone(), size, &text, false, TextDirection::Auto);
            surface.set_stroke(None);
        }
        Op::Rect { x, y, w, h, color: c } => {
            if let Some(r) = Rect::from_xywh(x, y, w, h) {
                let mut pb = PathBuilder::new();
                pb.push_rect(r);
                if let Some(path) = pb.finish() {
                    surface.set_fill(Some(fill(c)));
                    surface.draw_path(&path);
                }
            }
        }
        Op::Line { x1, y1, x2, y2, width, color: c } => {
            let mut pb = PathBuilder::new();
            pb.move_to(x1, y1);
            pb.line_to(x2, y2);
            if let Some(path) = pb.finish() {
                surface.set_fill(None);
                surface.set_stroke(Some(Stroke { paint: color(c).into(), width, ..Stroke::default() }));
                surface.draw_path(&path);
                surface.set_stroke(None);
            }
        }
    }
}

/// 中日韓文字與全形標點：每個字之間都能斷行。
fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

/// 不放在行首的標點（擠在上一行行尾）。
fn is_no_break_start(t: &str) -> bool {
    let mut cs = t.chars();
    matches!((cs.next(), cs.next()), (Some(c), None) if "，。、；：！？）」』》〉】,.;:!?)%".contains(c))
}

// ---------- 內容 ----------

/// 會議資訊表：左欄淡紫底的欄名、右欄內容（同 Word 的 info_table）。
fn info_table(l: &mut Layout<'_>, p: &Project) {
    let key_w = 95.0;
    let source = super::meta_lines(p)
        .into_iter()
        .find_map(|l| l.strip_prefix("原始檔案：").map(str::to_string))
        .unwrap_or_default();
    let mut items = vec![("原始檔案".to_string(), source)];
    for (k, v) in [("逐字稿模型", &p.models.stt), ("說話者辨識", &p.models.attribution), ("摘要模型", &p.models.summary)] {
        if !v.trim().is_empty() && !(k == "摘要模型" && p.summary.is_none()) {
            items.push((k.to_string(), v.clone()));
        }
    }
    let rows = items
        .into_iter()
        .map(|(k, v)| vec![(vec![span(&k, 9.5, MUTED, false)], Some(ACCENT_TINT)), (vec![span(&v, 9.5, TEXT, false)], None)])
        .collect();
    l.table(rows, &[key_w, BODY_W - key_w]);
}

/// 發言者：顏色圓點＋名字、發言時間、佔比、句數（同 Word 的 speaker_table）。
fn speaker_table(l: &mut Layout<'_>, p: &Project) {
    let w = [BODY_W - 3.0 * 85.0, 85.0, 85.0, 85.0];
    let head = |t: &str| (vec![span(t, 9.5, ACCENT_DARK, true)], Some(ACCENT_TINT));
    let mut rows = vec![vec![head("說話者"), head("發言時間"), head("佔比"), head("句數")]];
    let stats = p.speaker_stats();
    let total: f64 = stats.iter().map(|s| s.1).sum::<f64>().max(1e-9);
    for (id, secs, count) in stats {
        let idx = p.speaker(&id).map(|s| s.color).unwrap_or(0);
        let c = SPEAKER_COLORS[idx % SPEAKER_COLORS.len()];
        rows.push(vec![
            (vec![span("● ", 9.5, c, false), span(&p.speaker_name(Some(&id)), 9.5, TEXT, false)], None),
            (vec![span(&fmt_duration_zh(secs).replace(' ', ""), 9.5, TEXT, false)], None),
            (vec![span(&format!("{:.0}%", secs / total * 100.0), 9.5, TEXT, false)], None),
            (vec![span(&count.to_string(), 9.5, TEXT, false)], None),
        ]);
    }
    l.table(rows, &w);
    l.y += 3.0;
}

/// 摘要：Heading2–4、條列（懸掛縮排）、引言（左側主色直線）、表格、段落，間距同 Word 的樣式。
fn markdown(l: &mut Layout<'_>, md: &str, title: &str) {
    for block in md_blocks(md, title) {
        match block {
            MdBlock::Heading(level, text) => {
                let (size, color, before, after) = match level + 1 {
                    2 => (14.0, TEXT, 14.0, 5.0),
                    3 => (12.0, ACCENT_DARK, 12.0, 4.0),
                    _ => (11.0, TEXT, 10.0, 3.0),
                };
                l.space(before);
                l.need(size * LINE + 2.0 * 11.0 * LINE);
                l.para(&rich(text, size, color, true), 0.0, 0.0, after);
            }
            MdBlock::Item(marker, depth, rest) => {
                let left = 18.0 * (depth as f32 + 1.0);
                let hang = 15.0;
                let lines = l.wrap(&rich(rest, 11.0, TEXT, false), BODY_W - left);
                let marker_color = if marker == "•" { ACCENT } else { TEXT };
                for (i, line) in lines.into_iter().enumerate() {
                    let h = line.height();
                    l.need(h);
                    let top = l.y;
                    if i == 0 {
                        let m = l.wrap(&[span(&marker, 11.0, marker_color, false)], hang * 3.0);
                        if let Some(m) = m.into_iter().next() {
                            l.draw_line(m, MARGIN + left - hang, top);
                        }
                    }
                    l.draw_line(line, MARGIN + left, top);
                    l.y += h;
                }
                l.y += 3.0;
            }
            MdBlock::Quote(q) => {
                l.space(4.0);
                let lines = l.wrap(&rich(q, 11.0, MUTED, false), BODY_W - 12.0 - 8.0);
                for line in lines {
                    let h = line.height();
                    l.need(h);
                    let top = l.y;
                    l.ops().push(Op::Line { x1: MARGIN + 12.0, y1: top, x2: MARGIN + 12.0, y2: top + h, width: 2.25, color: ACCENT });
                    l.draw_line(line, MARGIN + 12.0 + 8.0, top);
                    l.y += h;
                }
                l.y += 4.0;
            }
            MdBlock::Table(rows) => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let w = BODY_W / cols as f32;
                let rows = rows
                    .iter()
                    .enumerate()
                    .map(|(r, cells)| {
                        (0..cols)
                            .map(|c| {
                                let text = cells.get(c).map(String::as_str).unwrap_or("");
                                let color = if r == 0 { ACCENT_DARK } else { TEXT };
                                (rich(text, 9.5, color, r == 0), (r == 0).then_some(ACCENT_TINT))
                            })
                            .collect()
                    })
                    .collect();
                l.table(rows, &vec![w; cols]);
                l.y += 3.0;
            }
            MdBlock::Para(t) => l.para(&rich(t, 11.0, TEXT, false), 0.0, 0.0, 5.0),
        }
    }
}

// ---------- 字型 ----------

struct Face {
    data: Arc<Vec<u8>>,
    index: u32,
    font: Font,
    upem: f32,
}

/// 主要的中文字型（＋有的話粗體），其後是補字用的符號字型（☐☑● 等中文字型沒有的字）。
struct Fonts {
    faces: Vec<Face>,
    /// 粗體在 `faces` 的位置（沒有就用描邊假粗體）。
    bold: Option<usize>,
    cache: std::sync::Mutex<HashMap<(char, bool), Glyph>>,
}

#[derive(Clone, Copy)]
struct Glyph {
    face: usize,
    /// 寬度（字級 1 pt 時）。
    advance: f32,
    synth_bold: bool,
}

impl Fonts {
    fn glyph(&self, c: char, bold: bool) -> Glyph {
        if let Some(g) = self.cache.lock().expect("font cache").get(&(c, bold)) {
            return *g;
        }
        let order: Vec<usize> = match (bold, self.bold) {
            (true, Some(b)) => std::iter::once(b).chain((0..self.faces.len()).filter(|i| *i != b)).collect(),
            _ => (0..self.faces.len()).filter(|i| Some(*i) != self.bold).collect(),
        };
        let found = order.iter().find_map(|&i| {
            let f = &self.faces[i];
            let face = ttf_parser::Face::parse(&f.data, f.index).ok()?;
            let gid = face.glyph_index(c)?;
            Some((i, face.glyph_hor_advance(gid).unwrap_or(0) as f32 / f.upem))
        });
        let (face, advance) = found.unwrap_or((0, if c.is_whitespace() { 0.3 } else { 1.0 }));
        let g = Glyph { face, advance, synth_bold: bold && Some(face) != self.bold };
        self.cache.lock().expect("font cache").insert((c, bold), g);
        g
    }

    fn width(&self, text: &str, size: f32, bold: bool) -> f32 {
        text.chars().map(|c| self.glyph(c, bold).advance * size).sum()
    }
}

/// 字型只讀一次（中文字型 20 MB 上下）。
fn fonts() -> &'static std::result::Result<Fonts, String> {
    static FONTS: OnceLock<std::result::Result<Fonts, String>> = OnceLock::new();
    FONTS.get_or_init(load_fonts)
}

fn load_fonts() -> std::result::Result<Fonts, String> {
    let (main, bold, extra) = font_candidates();
    let mut faces = Vec::new();
    let push = |path: &PathBuf, faces: &mut Vec<Face>| -> bool {
        let Ok(bytes) = std::fs::read(path) else { return false };
        let data = Arc::new(bytes);
        let index = pick_face(&data);
        let Ok(face) = ttf_parser::Face::parse(&data, index) else { return false };
        let upem = face.units_per_em() as f32;
        let Some(font) = Font::new(data.clone().into(), index) else { return false };
        faces.push(Face { data, index, font, upem });
        true
    };
    if !main.iter().any(|p| push(p, &mut faces)) {
        return Err(format!("找不到中文字型（{}），無法產生 PDF", FONT_EA));
    }
    let bold = bold.iter().any(|p| push(p, &mut faces)).then_some(1);
    for p in &extra {
        push(p, &mut faces);
    }
    Ok(Fonts { faces, bold, cache: Default::default() })
}

/// 字型集合（.ttc）裡挑繁體中文那一個：Noto Sans CJK 的「TC」、微軟正黑體不是「UI」的那個。
fn pick_face(data: &[u8]) -> u32 {
    let n = ttf_parser::fonts_in_collection(data).unwrap_or(1);
    let family = |i: u32| -> String {
        ttf_parser::Face::parse(data, i)
            .ok()
            .and_then(|f| {
                f.names()
                    .into_iter()
                    .filter(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
                    .find_map(|n| n.to_string())
            })
            .unwrap_or_default()
    };
    let names: Vec<String> = (0..n).map(family).collect();
    names
        .iter()
        .position(|f| f.contains(" TC"))
        .or_else(|| names.iter().position(|f| f.contains("JhengHei") && !f.contains("UI")))
        .unwrap_or(0) as u32
}

/// (主要字型, 粗體, 補字字型) 的候選路徑，依序找第一個存在的。`MEETSCRIBE_PDF_FONT` 可以指定主要字型。
fn font_candidates() -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    let mut main: Vec<PathBuf> = std::env::var_os("MEETSCRIBE_PDF_FONT").map(PathBuf::from).into_iter().collect();
    let (dir, m, b, e): (PathBuf, &[&str], &[&str], &[&str]) = if cfg!(target_os = "android") {
        (
            PathBuf::from("/system/fonts"),
            &["NotoSansCJK-Regular.ttc", "NotoSansCJKtc-Regular.otf", "NotoSerifCJK-Regular.ttc", "DroidSansFallbackFull.ttf", "DroidSansFallback.ttf"],
            &["NotoSansCJK-Bold.ttc"],
            &["Roboto-Regular.ttf", "NotoSansSymbols-Regular-Subsetted.ttf", "NotoSansSymbols-Regular-Subsetted2.ttf"],
        )
    } else if cfg!(windows) {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        (windir.join("Fonts"), &["msjh.ttc", "msjh.ttf", "mingliu.ttc", "simsun.ttc"], &["msjhbd.ttc", "msjhbd.ttf"], &["seguisym.ttf", "segoeui.ttf"])
    } else {
        (
            PathBuf::from("/usr/share/fonts"),
            &["opentype/noto/NotoSansCJK-Regular.ttc", "noto-cjk/NotoSansCJK-Regular.ttc", "google-noto-cjk/NotoSansCJK-Regular.ttc"],
            &["opentype/noto/NotoSansCJK-Bold.ttc", "noto-cjk/NotoSansCJK-Bold.ttc"],
            &["truetype/dejavu/DejaVuSans.ttf"],
        )
    };
    main.extend(m.iter().map(|f| dir.join(f)));
    (main, b.iter().map(|f| dir.join(f)).collect(), e.iter().map(|f| dir.join(f)).collect())
}

#[cfg(test)]
pub(super) fn font_available() -> bool {
    fonts().is_ok()
}

#[cfg(test)]
pub(super) fn line_breaks(text: &str, width: f32) -> Vec<String> {
    let f = fonts().as_ref().expect("font");
    Layout::new(f)
        .wrap(&[span(text, 11.0, TEXT, false)], width)
        .into_iter()
        .map(|l| l.pieces.into_iter().map(|p| p.text).collect())
        .collect()
}
