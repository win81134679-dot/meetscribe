//! 會議摘要：格式不固定，由模型依會議性質決定。超長逐字稿先分段做筆記再合併（map-reduce）。

use crate::config::Settings;
use crate::model::{Project, Summary};
use crate::prompts::{Prompts, fill};
use crate::providers::{ChatMessage, LlmClient, LlmRequest, LlmResponse, Role};
use crate::textutil::{fmt_duration_zh, fmt_hms};
use crate::{Error, Result};
use chrono::Local;
use tokio_util::sync::CancellationToken;

/// `[00:01:02] 王經理：……`，同一人連續發言合成一行。
pub fn transcript_for_llm(project: &Project) -> String {
    let mut out = String::new();
    let mut cur_speaker: Option<&str> = None;
    for seg in &project.segments {
        let sp = seg.speaker.as_deref();
        if cur_speaker == Some(sp.unwrap_or("")) && !out.is_empty() {
            if crate::model::needs_space(&out, &seg.text) {
                out.push(' ');
            }
            out.push_str(&seg.text);
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let name = if project.options.identify_speakers {
            project.speaker_name(sp)
        } else {
            String::new()
        };
        if name.is_empty() {
            out.push_str(&format!("[{}] {}", fmt_hms(seg.start), seg.text));
        } else {
            out.push_str(&format!("[{}] {}：{}", fmt_hms(seg.start), name, seg.text));
        }
        cur_speaker = Some(sp.unwrap_or(""));
    }
    out
}

/// 在行邊界切成每份不超過 `max_chars` 字。
pub fn split_lines(text: &str, max_chars: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut buf = String::new();
    let mut len = 0usize;
    for line in text.lines() {
        let n = line.chars().count() + 1;
        if len + n > max_chars && !buf.is_empty() {
            parts.push(std::mem::take(&mut buf));
            len = 0;
        }
        buf.push_str(line);
        buf.push('\n');
        len += n;
    }
    if !buf.trim().is_empty() {
        parts.push(buf);
    }
    parts
}

fn header(project: &Project) -> String {
    let mut h = format!(
        "會議檔名：{}\n長度：{}\n",
        project.title,
        fmt_duration_zh(project.duration)
    );
    if project.options.identify_speakers && !project.speakers.is_empty() {
        h.push_str(&format!("發言人數：{}\n", project.speakers.len()));
    }
    if !project.options.participants.is_empty() {
        h.push_str(&format!("與會者名單：{}\n", project.options.participants.join("、")));
    }
    h
}

pub struct SummaryOutcome {
    pub summary: Summary,
    pub cost: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub async fn summarize(
    project: &Project,
    llm: &dyn LlmClient,
    settings: &Settings,
    prompts: &Prompts,
    instructions: &str,
    progress: &(dyn Fn(f32, String) + Send + Sync),
    cancel: &CancellationToken,
) -> Result<SummaryOutcome> {
    let transcript = transcript_for_llm(project);
    if transcript.trim().is_empty() {
        return Err(Error::Other("逐字稿是空的，無法摘要".into()));
    }
    let system = fill(&prompts.summary, &[("language", &settings.output_language_label())]);
    let extra = if instructions.trim().is_empty() {
        String::new()
    } else {
        format!("\n使用者對這份摘要的額外要求（優先遵守）：{}\n", instructions.trim())
    };
    let model = settings.summary_model.clone();
    let mut usage = (0.0, 0u64, 0u64);
    let mut add = |r: &LlmResponse| {
        usage.0 += r.cost;
        usage.1 += r.input_tokens;
        usage.2 += r.output_tokens;
    };

    let body = if transcript.chars().count() <= settings.summary_single_pass_chars {
        progress(0.1, "整份逐字稿一次摘要".into());
        format!("{}{}\n逐字稿：\n{}", header(project), extra, transcript)
    } else {
        let parts = split_lines(&transcript, settings.summary_single_pass_chars / 2);
        let n = parts.len();
        let mut notes = Vec::with_capacity(n);
        for (i, part) in parts.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            progress(i as f32 / (n + 1) as f32, format!("長會議分段筆記 {}/{}", i + 1, n));
            let range = range_of(part);
            let prompt = fill(
                &prompts.summary_map,
                &[("i", &(i + 1).to_string()), ("n", &n.to_string()), ("range", &range)],
            );
            let r = llm
                .complete(
                    LlmRequest {
                        model: model.clone(),
                        messages: vec![
                            ChatMessage::text(Role::System, system.clone()),
                            ChatMessage::text(Role::User, format!("{prompt}\n\n{part}")),
                        ],
                        temperature: Some(0.2),
                        max_tokens: Some(8000),
                    },
                    cancel,
                )
                .await?;
            add(&r);
            notes.push(format!("### 第 {}/{} 部分（{}）\n{}", i + 1, n, range, r.text.trim()));
        }
        progress(n as f32 / (n + 1) as f32, "合併各部分筆記".into());
        format!(
            "{}{}\n{}\n\n{}",
            header(project),
            extra,
            prompts.summary_reduce,
            notes.join("\n\n")
        )
    };

    let r = llm
        .complete(
            LlmRequest {
                model: model.clone(),
                messages: vec![
                    ChatMessage::text(Role::System, system),
                    ChatMessage::text(Role::User, body),
                ],
                temperature: Some(0.2),
                max_tokens: Some(16000),
            },
            cancel,
        )
        .await?;
    add(&r);
    let mut markdown = strip_outer_fence(r.text.trim()).to_string();
    if r.truncated() {
        markdown.push_str("\n\n> ⚠ 摘要超過模型輸出上限被截斷，可在「摘要」頁重新產生或換模型。");
    }
    progress(1.0, "摘要完成".into());
    Ok(SummaryOutcome {
        summary: Summary {
            markdown,
            model,
            created_at: Local::now(),
            instructions: instructions.trim().to_string(),
        },
        cost: usage.0,
        input_tokens: usage.1,
        output_tokens: usage.2,
    })
}

/// 第一行與最後一行的時間標記。
fn range_of(part: &str) -> String {
    let stamp = |l: &str| {
        l.strip_prefix('[')
            .and_then(|r| r.split(']').next())
            .map(str::to_string)
    };
    let first = part.lines().find_map(stamp).unwrap_or_default();
    let last = part.lines().rev().find_map(stamp).unwrap_or_default();
    format!("{first}～{last}")
}

/// 有些模型會把整份 Markdown 包在 ```markdown 裡。
fn strip_outer_fence(s: &str) -> &str {
    let Some(rest) = s.strip_prefix("```") else { return s };
    let Some(nl) = rest.find('\n') else { return s };
    rest[nl + 1..]
        .trim_end()
        .strip_suffix("```")
        .map(str::trim)
        .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{JobOptions, Segment, Timing};

    fn project() -> Project {
        let mut p = Project::new("週會.mp3".into(), JobOptions::default());
        p.add_speaker(Some("王經理".into()), String::new());
        p.add_speaker(None, String::new());
        let mk = |id, start: f64, sp: &str, text: &str| Segment {
            id,
            start,
            end: start + 2.0,
            timing: Timing::Model,
            speaker: Some(sp.into()),
            text: text.into(),
            chunk: 0,
            edited: false,
        };
        p.segments = vec![
            mk(1, 0.0, "S1", "開始吧。"),
            mk(2, 2.0, "S1", "先看預算。"),
            mk(3, 65.0, "S2", "好的。"),
        ];
        p
    }

    #[test]
    fn transcript_merges_consecutive_turns() {
        let t = transcript_for_llm(&project());
        assert_eq!(t, "[00:00:00] 王經理：開始吧。先看預算。\n[00:01:05] 說話者 2：好的。");
    }

    #[test]
    fn split_respects_lines() {
        let parts = split_lines("aaaa\nbbbb\ncccc\n", 10);
        assert_eq!(parts, vec!["aaaa\nbbbb\n", "cccc\n"]);
        assert_eq!(range_of("[00:00:01] a\n[00:05:00] b\n"), "00:00:01～00:05:00");
    }

    #[test]
    fn fence_stripping() {
        assert_eq!(strip_outer_fence("```markdown\n# 標題\n```"), "# 標題");
        assert_eq!(strip_outer_fence("# 標題"), "# 標題");
    }
}
