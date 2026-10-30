//! 專案（一場會議）的資料模型。整份序列化成 `project.json`。

use crate::export::ExportOptions;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

pub const PROJECT_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProjectStatus {
    Queued,
    Running,
    Done,
    Failed(String),
    Cancelled,
}

impl ProjectStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ProjectStatus::Queued => "排隊中",
            ProjectStatus::Running => "處理中",
            ProjectStatus::Done => "完成",
            ProjectStatus::Failed(_) => "失敗",
            ProjectStatus::Cancelled => "已取消",
        }
    }
    /// 中斷過、可以從斷點續跑。
    pub fn is_resumable(&self) -> bool {
        matches!(self, ProjectStatus::Failed(_) | ProjectStatus::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Prepare,
    Transcribe,
    Speakers,
    Reconcile,
    Summary,
    Done,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Prepare => "前處理音訊",
            Stage::Transcribe => "語音轉文字",
            Stage::Speakers => "辨識說話者",
            Stage::Reconcile => "整合說話者",
            Stage::Summary => "產生摘要",
            Stage::Done => "完成",
        }
    }
    pub const ALL: [Stage; 5] = [
        Stage::Prepare,
        Stage::Transcribe,
        Stage::Speakers,
        Stage::Reconcile,
        Stage::Summary,
    ];
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct JobOptions {
    /// ISO-639-1；`None` 讓模型自動判斷。
    pub language: Option<String>,
    pub identify_speakers: bool,
    /// 使用者預估的發言人數（只當提示用）。
    pub expected_speakers: Option<u32>,
    /// 與會者名單，一行一位，可含職稱。
    pub participants: Vec<String>,
    /// 專有名詞、產品代號、人名，提升辨識率。
    pub keyterms: Vec<String>,
    pub generate_summary: bool,
    /// 摘要的額外指示（例如「著重技術決策」）。
    pub summary_instructions: String,
    /// 這場會議的輸出選項（建立時從設定複製）；`None` 用目前設定（舊專案）。
    pub export: Option<ExportOptions>,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            language: None,
            identify_speakers: true,
            expected_speakers: None,
            participants: Vec::new(),
            keyterms: Vec::new(),
            generate_summary: true,
            summary_instructions: String::new(),
            export: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TimedWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub speaker: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TimedSegment {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub speaker: Option<String>,
}

/// 一段音訊的語音辨識結果（時間相對於該段開頭）。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SttResult {
    pub text: String,
    pub language: Option<String>,
    pub words: Vec<TimedWord>,
    pub segments: Vec<TimedSegment>,
    pub cost: f64,
    pub seconds: f64,
}

impl SttResult {
    pub fn has_speaker_labels(&self) -> bool {
        self.segments.iter().any(|s| s.speaker.is_some()) || self.words.iter().any(|w| w.speaker.is_some())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChunkRecord {
    pub index: usize,
    /// 在整份音訊中的起訖秒數。
    pub start: f64,
    pub end: f64,
    /// 相對於專案資料夾。
    pub file: String,
    pub stt: Option<SttResult>,
    /// 這段的說話者分配已完成。
    #[serde(default)]
    pub attributed: bool,
}

impl ChunkRecord {
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NameSource {
    /// 程式給的預設標籤（說話者 1）。
    Label,
    /// 模型從對話內容推測的。
    Inferred,
    /// 使用者自己改的，模型不會再覆蓋。
    User,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Speaker {
    pub id: String,
    pub name: String,
    pub name_source: NameSource,
    pub color: usize,
    /// 聲音特徵或角色描述（模型給的，給下一段比對用）。
    #[serde(default)]
    pub note: String,
    /// 聲音樣本（相對於專案資料夾）。
    #[serde(default)]
    pub sample_clip: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Timing {
    /// 語音辨識供應商給的時間戳。
    Provider,
    /// 說話者分配模型聽音訊給的時間。
    Model,
    /// 依字數比例估算。
    Estimated,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub id: u64,
    pub start: f64,
    pub end: f64,
    pub timing: Timing,
    pub speaker: Option<String>,
    pub text: String,
    pub chunk: usize,
    #[serde(default)]
    pub edited: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub markdown: String,
    pub model: String,
    pub created_at: DateTime<Local>,
    #[serde(default)]
    pub instructions: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Usage {
    pub stt_usd: f64,
    pub stt_seconds: f64,
    pub llm_usd: f64,
    pub llm_input_tokens: u64,
    pub llm_output_tokens: u64,
}

impl Usage {
    pub fn total_usd(&self) -> f64 {
        self.stt_usd + self.llm_usd
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelsUsed {
    pub stt: String,
    pub attribution: String,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub version: u32,
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Local>,
    pub updated_at: DateTime<Local>,
    pub source_file: PathBuf,
    /// 正規化後的 16 kHz mono MP3（相對於專案資料夾）。
    pub audio_file: Option<String>,
    pub duration: f64,
    pub options: JobOptions,
    pub status: ProjectStatus,
    pub stage: Stage,
    pub chunks: Vec<ChunkRecord>,
    pub speakers: Vec<Speaker>,
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub speakers_reconciled: bool,
    pub summary: Option<Summary>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub models: ModelsUsed,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// 最近一次自動匯出寫出的檔案。
    #[serde(default)]
    pub exported: Vec<PathBuf>,
}

impl Project {
    pub fn new(source_file: PathBuf, options: JobOptions) -> Self {
        let now = Local::now();
        let title = source_file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名會議".into());
        Self {
            version: PROJECT_FORMAT_VERSION,
            id: Uuid::new_v4(),
            title,
            created_at: now,
            updated_at: now,
            source_file,
            audio_file: None,
            duration: 0.0,
            options,
            status: ProjectStatus::Queued,
            stage: Stage::Prepare,
            chunks: Vec::new(),
            speakers: Vec::new(),
            segments: Vec::new(),
            speakers_reconciled: false,
            summary: None,
            usage: Usage::default(),
            models: ModelsUsed::default(),
            warnings: Vec::new(),
            exported: Vec::new(),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Local::now();
    }

    pub fn speaker(&self, id: &str) -> Option<&Speaker> {
        self.speakers.iter().find(|s| s.id == id)
    }

    pub fn speaker_mut(&mut self, id: &str) -> Option<&mut Speaker> {
        self.speakers.iter_mut().find(|s| s.id == id)
    }

    pub fn speaker_name(&self, id: Option<&str>) -> String {
        match id.and_then(|id| self.speaker(id)) {
            Some(s) => s.name.clone(),
            None => "（未標註）".into(),
        }
    }

    /// 下一個沒用過的說話者 ID（S1、S2…）。
    pub fn next_speaker_id(&self) -> String {
        let max = self
            .speakers
            .iter()
            .filter_map(|s| s.id.strip_prefix('S').and_then(|n| n.parse::<u32>().ok()))
            .max()
            .unwrap_or(0);
        format!("S{}", max + 1)
    }

    /// 新增一位說話者並回傳 ID。
    pub fn add_speaker(&mut self, name: Option<String>, note: String) -> String {
        let id = self.next_speaker_id();
        let number = id.trim_start_matches('S').to_string();
        let (name, source) = match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => (n.trim().to_string(), NameSource::Inferred),
            None => (format!("說話者 {number}"), NameSource::Label),
        };
        let color = self.speakers.len();
        self.speakers.push(Speaker {
            id: id.clone(),
            name,
            name_source: source,
            color,
            note,
            sample_clip: None,
        });
        id
    }

    pub fn rename_speaker(&mut self, id: &str, name: &str) {
        if let Some(s) = self.speaker_mut(id) {
            s.name = name.trim().to_string();
            s.name_source = NameSource::User;
        }
    }

    /// 把 `from` 的所有發言改給 `into`，並刪除 `from`。
    pub fn merge_speakers(&mut self, from: &str, into: &str) -> bool {
        if from == into || self.speaker(from).is_none() || self.speaker(into).is_none() {
            return false;
        }
        for seg in &mut self.segments {
            if seg.speaker.as_deref() == Some(from) {
                seg.speaker = Some(into.to_string());
            }
        }
        let from_sp = self.speaker(from).cloned();
        self.speakers.retain(|s| s.id != from);
        // 被併入的人沒有名字或樣本時，接收來源的。
        if let (Some(from_sp), Some(target)) = (from_sp, self.speaker_mut(into)) {
            if target.name_source == NameSource::Label && from_sp.name_source != NameSource::Label {
                target.name = from_sp.name;
                target.name_source = from_sp.name_source;
            }
            if target.sample_clip.is_none() {
                target.sample_clip = from_sp.sample_clip;
            }
        }
        true
    }

    pub fn next_segment_id(&self) -> u64 {
        self.segments.iter().map(|s| s.id).max().map_or(1, |m| m + 1)
    }

    /// 每位說話者的（發言秒數、句數），依發言秒數由多到少。
    pub fn speaker_stats(&self) -> Vec<(String, f64, usize)> {
        let mut map: HashMap<&str, (f64, usize)> = HashMap::new();
        for seg in &self.segments {
            if let Some(sp) = &seg.speaker {
                let e = map.entry(sp.as_str()).or_default();
                e.0 += (seg.end - seg.start).max(0.0);
                e.1 += 1;
            }
        }
        let mut out: Vec<(String, f64, usize)> = self
            .speakers
            .iter()
            .map(|s| {
                let (t, c) = map.get(s.id.as_str()).copied().unwrap_or_default();
                (s.id.clone(), t, c)
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    /// 刪掉沒有任何發言的說話者（合併或改句之後）。
    pub fn prune_unused_speakers(&mut self) {
        let used: std::collections::HashSet<String> = self.segments.iter().filter_map(|s| s.speaker.clone()).collect();
        self.speakers.retain(|s| used.contains(&s.id));
    }

    pub fn sort_segments(&mut self) {
        self.segments
            .sort_by(|a, b| a.start.total_cmp(&b.start).then(a.id.cmp(&b.id)));
    }

    /// 把第 `idx` 句併進前一句（同一位說話者時最自然）。
    pub fn merge_segment_with_previous(&mut self, idx: usize) -> bool {
        if idx == 0 || idx >= self.segments.len() {
            return false;
        }
        let cur = self.segments.remove(idx);
        let prev = &mut self.segments[idx - 1];
        let sep = if needs_space(&prev.text, &cur.text) { " " } else { "" };
        prev.text = format!("{}{}{}", prev.text, sep, cur.text);
        prev.end = prev.end.max(cur.end);
        prev.edited = true;
        true
    }

    pub fn is_finished(&self) -> bool {
        self.status == ProjectStatus::Done
    }
}

/// 兩段文字接起來時要不要補空白（拉丁字母之間要，中日韓不要）。
pub fn needs_space(a: &str, b: &str) -> bool {
    let last = a.chars().last();
    let first = b.chars().next();
    // 英文字接英文字，或英文句讀（. , ! ? ; :）後面接英文字（"Thanks." + "I am…"）
    matches!((last, first), (Some(x), Some(y))
        if (x.is_ascii_alphanumeric() || matches!(x, '.' | ',' | '!' | '?' | ';' | ':')) && y.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: u64, start: f64, end: f64, sp: &str, text: &str) -> Segment {
        Segment {
            id,
            start,
            end,
            timing: Timing::Model,
            speaker: Some(sp.into()),
            text: text.into(),
            chunk: 0,
            edited: false,
        }
    }

    #[test]
    fn add_and_merge_speakers() {
        let mut p = Project::new("a.wav".into(), JobOptions::default());
        let a = p.add_speaker(None, String::new());
        let b = p.add_speaker(Some("王經理".into()), String::new());
        assert_eq!((a.as_str(), b.as_str()), ("S1", "S2"));
        assert_eq!(p.speaker("S1").unwrap().name, "說話者 1");
        p.segments = vec![seg(1, 0.0, 5.0, "S1", "大家好"), seg(2, 5.0, 7.0, "S2", "好")];
        assert!(p.merge_speakers("S2", "S1"));
        assert!(p.segments.iter().all(|s| s.speaker.as_deref() == Some("S1")));
        // 被併入的人沒有名字，接收推測的名字
        assert_eq!(p.speaker("S1").unwrap().name, "王經理");
        assert_eq!(p.next_speaker_id(), "S2");
        assert!(!p.merge_speakers("S1", "S1"));
    }

    #[test]
    fn stats_sorted_by_talk_time() {
        let mut p = Project::new("a.wav".into(), JobOptions::default());
        p.add_speaker(None, String::new());
        p.add_speaker(None, String::new());
        p.segments = vec![seg(1, 0.0, 2.0, "S1", "a"), seg(2, 2.0, 10.0, "S2", "b")];
        let stats = p.speaker_stats();
        assert_eq!(stats[0].0, "S2");
        assert!((stats[0].1 - 8.0).abs() < 1e-9);
    }

    #[test]
    fn merge_segment_joins_text() {
        let mut p = Project::new("a.wav".into(), JobOptions::default());
        p.segments = vec![seg(1, 0.0, 2.0, "S1", "hello"), seg(2, 2.0, 4.0, "S1", "world")];
        assert!(p.merge_segment_with_previous(1));
        assert_eq!(p.segments.len(), 1);
        assert_eq!(p.segments[0].text, "hello world");
        assert_eq!(p.segments[0].end, 4.0);
    }

    #[test]
    fn english_sentences_joined_with_space() {
        // 匯出時同一人連續發言併段：英文句號後面接英文要空一格
        assert!(needs_space("Thanks.", "I am Linda"));
        assert!(needs_space("Really?", "Yes"));
        assert!(needs_space("one,", "two"));
        assert!(needs_space("hello", "world"));
        // 中文、標點開頭、空白結尾都不加
        assert!(!needs_space("大家好。", "今天"));
        assert!(!needs_space("Thanks.", "大家好"));
        assert!(!needs_space("Hello", ","));
        assert!(!needs_space("end. ", "Next"));
    }
}
