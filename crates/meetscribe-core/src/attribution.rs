//! 說話者辨識。
//!
//! 1. **分配**：每段音訊連同已切好編號的逐字稿送給能聽音訊的模型，只回 `[句號, 說話者, 起始秒]`。
//!    已知說話者附聲音樣本，模型沿用舊 ID；新人用 N1、N2，由程式配發全域 ID。
//! 2. **整合**：全部段落分完後，文字模型看每位說話者的描述與發言樣本，保守地合併重複 ID、推測姓名。
//!
//! 逐字稿文字永遠來自語音辨識，模型漏回的句子沿用前一位說話者、時間用內插補上。

use crate::model::{NameSource, Project};
use crate::prompts::Prompts;
use crate::providers::{ChatMessage, ContentPart, LlmRequest, Role};
use crate::textutil::{extract_json, fmt_clock, normalize_speaker_id};
use crate::{Error, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// 傳給模型的已知說話者。
#[derive(Clone, Debug)]
pub struct KnownSpeaker {
    pub id: String,
    pub name: Option<String>,
    pub note: String,
    pub clip: Option<Arc<Vec<u8>>>,
}

pub struct AttributionInput<'a> {
    pub model: &'a str,
    pub audio: Arc<Vec<u8>>,
    pub audio_format: &'a str,
    pub chunk_index: usize,
    pub chunk_count: usize,
    pub chunk_start: f64,
    pub units: &'a [String],
    pub known: &'a [KnownSpeaker],
    /// 名單太長時只列前面幾位，這是總數。
    pub known_total: usize,
    pub participants: &'a [String],
    pub expected_speakers: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerInfo {
    pub id: String,
    pub name: Option<String>,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub unit: usize,
    pub speaker: String,
    pub t: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedAttribution {
    pub speakers: Vec<SpeakerInfo>,
    pub assignments: Vec<Assignment>,
}

/// 名單最多列幾位（1000 人的大會只列發言最多的）。
pub const ROSTER_LIMIT: usize = 150;

pub fn build_request(prompts: &Prompts, input: &AttributionInput<'_>) -> LlmRequest {
    let mut ctx = String::new();
    ctx.push_str(&format!(
        "這是整場會議的第 {}/{} 段音訊，從整場的 {} 開始。\n",
        input.chunk_index + 1,
        input.chunk_count,
        fmt_clock(input.chunk_start)
    ));
    if let Some(n) = input.expected_speakers {
        ctx.push_str(&format!("使用者預估整場大約有 {n} 位發言者（僅供參考）。\n"));
    }
    if !input.participants.is_empty() {
        ctx.push_str("與會者名單（不一定每位都會發言）：\n");
        for p in input.participants.iter().take(300) {
            ctx.push_str(&format!("- {p}\n"));
        }
    }
    if input.known.is_empty() {
        ctx.push_str("\n已知說話者：無（這是第一段，所有人都用 N1、N2… 標註）。\n");
    } else {
        ctx.push_str("\n已知說話者（前面段落已出現，沿用這些 ID）：\n");
        for k in input.known {
            let name = k.name.as_deref().unwrap_or("姓名未知");
            let clip = if k.clip.is_some() { "，附聲音樣本" } else { "" };
            ctx.push_str(&format!("- {}：{}；{}{}\n", k.id, name, k.note, clip));
        }
        if input.known_total > input.known.len() {
            ctx.push_str(&format!(
                "（另有 {} 位發言較少的說話者未列出；若無法對應已列出的人，就當新的人用 N 編號。）\n",
                input.known_total - input.known.len()
            ));
        }
    }

    let mut content = vec![ContentPart::Text(ctx)];
    for k in input.known.iter().filter(|k| k.clip.is_some()) {
        content.push(ContentPart::Text(format!("聲音樣本 {}：", k.id)));
        content.push(ContentPart::Audio {
            data: k.clip.clone().expect("filtered"),
            format: input.audio_format.into(),
        });
    }
    content.push(ContentPart::Text("要標註的這段音訊：".into()));
    content.push(ContentPart::Audio {
        data: input.audio.clone(),
        format: input.audio_format.into(),
    });

    let mut numbered = format!(
        "逐字稿（共 {} 句，編號 0～{}）：\n",
        input.units.len(),
        input.units.len().saturating_sub(1)
    );
    for (i, u) in input.units.iter().enumerate() {
        numbered.push_str(&format!("[{i}] {u}\n"));
    }
    content.push(ContentPart::Text(numbered));

    LlmRequest {
        model: input.model.to_string(),
        messages: vec![
            ChatMessage::text(Role::System, prompts.attribution.clone()),
            ChatMessage {
                role: Role::User,
                content,
            },
        ],
        temperature: Some(0.0),
        // 每句約 12 token，留足餘裕
        max_tokens: Some((input.units.len() as u32 * 24 + 2048).min(64_000)),
    }
}

/// 解析模型輸出。接受 `{"speakers":[...],"a":[[i,"S1",t],...]}`，
/// 也接受 `a` 是物件陣列 `{"i":0,"s":"S1","t":1.2}`（有些模型會這樣回）。
pub fn parse_response(raw: &str) -> Result<ParsedAttribution> {
    let json = extract_json(raw).ok_or_else(|| Error::BadModelOutput("找不到 JSON".into()))?;
    let v: Value = serde_json::from_str(json).map_err(|e| Error::BadModelOutput(e.to_string()))?;
    let mut speakers = Vec::new();
    if let Some(arr) = v["speakers"].as_array() {
        for s in arr {
            let Some(id) = s["id"].as_str().and_then(normalize_speaker_id) else {
                continue;
            };
            speakers.push(SpeakerInfo {
                id,
                name: s["name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|n| !n.is_empty() && *n != "null")
                    .map(str::to_string),
                note: s["note"].as_str().unwrap_or_default().trim().to_string(),
            });
        }
    }
    let list = v["a"]
        .as_array()
        .or_else(|| v["assignments"].as_array())
        .ok_or_else(|| Error::BadModelOutput("缺少 a 陣列".into()))?;
    let mut assignments = Vec::with_capacity(list.len());
    for item in list {
        let (i, s, t) = match item {
            Value::Array(x) => (x.first(), x.get(1), x.get(2)),
            Value::Object(_) => (item.get("i"), item.get("s").or(item.get("speaker")), item.get("t")),
            _ => continue,
        };
        let Some(unit) = i.and_then(Value::as_u64) else {
            continue;
        };
        let Some(speaker) = s.and_then(|s| match s {
            Value::String(x) => normalize_speaker_id(x),
            Value::Number(n) => normalize_speaker_id(&n.to_string()),
            _ => None,
        }) else {
            continue;
        };
        let t = t.and_then(|t| t.as_f64().or_else(|| t.as_str().and_then(parse_time)));
        assignments.push(Assignment {
            unit: unit as usize,
            speaker,
            t,
        });
    }
    Ok(ParsedAttribution { speakers, assignments })
}

/// `83.5`、`01:23.5`、`1:01:23` 都接受。
fn parse_time(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    let mut acc = 0.0;
    for p in &parts {
        acc = acc * 60.0 + p.parse::<f64>().ok()?;
    }
    Some(acc)
}

/// 每句的說話者原始 ID 與模型給的時間；缺的沿用前一位（開頭缺的用後一位）。
/// 回傳 `(每句說話者, 每句時間, 模型實際涵蓋的句數)`。
pub fn resolve(parsed: &ParsedAttribution, n_units: usize) -> (Vec<String>, Vec<Option<f64>>, usize) {
    let mut speakers: Vec<Option<String>> = vec![None; n_units];
    let mut times: Vec<Option<f64>> = vec![None; n_units];
    for a in &parsed.assignments {
        if a.unit < n_units && speakers[a.unit].is_none() {
            speakers[a.unit] = Some(a.speaker.clone());
            times[a.unit] = a.t;
        }
    }
    let covered = speakers.iter().filter(|s| s.is_some()).count();
    let first = speakers.iter().flatten().next().cloned().unwrap_or_else(|| "N1".into());
    let mut last = first;
    let filled = speakers
        .into_iter()
        .map(|s| {
            if let Some(s) = s {
                last = s;
            }
            last.clone()
        })
        .collect();
    (filled, times, covered)
}

/// 用模型給的時間當錨點（取最長非遞減子序列，剔除亂跳的值），其餘依字數內插。
/// 回傳每句 `(開始, 結束)`，相對於本段開頭，首尾相接。
pub fn fill_times(weights: &[usize], anchors: &[Option<f64>], duration: f64) -> Vec<(f64, f64)> {
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    let valid: Vec<(usize, f64)> = anchors
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            t.filter(|t| t.is_finite() && *t >= 0.0 && *t <= duration + 1.0)
                .map(|t| (i, t.min(duration)))
        })
        .collect();
    let keep = longest_non_decreasing(&valid);
    let mut starts: Vec<Option<f64>> = vec![None; n];
    for (i, t) in keep {
        starts[i] = Some(t);
    }
    if starts[0].is_none() {
        starts[0] = Some(0.0);
    }
    // 在兩個錨點之間依字數比例內插
    let mut i = 0;
    while i < n {
        let s = starts[i].expect("anchored");
        let mut j = i + 1;
        while j < n && starts[j].is_none() {
            j += 1;
        }
        let end = if j < n { starts[j].expect("anchored") } else { duration };
        let total: usize = weights[i..j].iter().sum::<usize>().max(1);
        let mut acc = 0usize;
        for k in i..j {
            starts[k] = Some(s + (end - s) * acc as f64 / total as f64);
            acc += weights[k];
        }
        i = j;
    }
    let starts: Vec<f64> = starts.into_iter().map(|s| s.unwrap_or(0.0)).collect();
    (0..n)
        .map(|k| {
            (
                starts[k],
                if k + 1 < n {
                    starts[k + 1]
                } else {
                    duration.max(starts[k])
                },
            )
        })
        .collect()
}

fn longest_non_decreasing(points: &[(usize, f64)]) -> Vec<(usize, f64)> {
    if points.is_empty() {
        return Vec::new();
    }
    // tails[k] = 長度 k+1 的子序列結尾最小值所在的 points 索引
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; points.len()];
    for (idx, &(_, t)) in points.iter().enumerate() {
        let pos = tails.partition_point(|&ti| points[ti].1 <= t);
        if pos > 0 {
            prev[idx] = Some(tails[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(idx);
        } else {
            tails[pos] = idx;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(c) = cur {
        out.push(points[c]);
        cur = prev[c];
    }
    out.reverse();
    out
}

/// 把模型在本段用的 ID（已知的 S#、新的 N#、或亂編的 S#）對應到專案的全域 ID，必要時新增說話者。
pub fn map_to_global(project: &mut Project, parsed: &ParsedAttribution, raw_ids: &[String]) -> HashMap<String, String> {
    let info: HashMap<&str, &SpeakerInfo> = parsed.speakers.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut map = HashMap::new();
    for raw in raw_ids {
        if map.contains_key(raw) {
            continue;
        }
        let meta = info.get(raw.as_str());
        let global = if raw.starts_with('S') && project.speaker(raw).is_some() {
            raw.clone()
        } else {
            let name = meta.and_then(|m| m.name.clone());
            let note = meta.map(|m| m.note.clone()).unwrap_or_default();
            project.add_speaker(name, note)
        };
        // 已知說話者：補上名字與描述（不覆蓋使用者改過的）
        if let (Some(m), Some(sp)) = (meta, project.speaker_mut(&global)) {
            if sp.note.is_empty() && !m.note.is_empty() {
                sp.note = m.note.clone();
            }
            if sp.name_source == NameSource::Label
                && let Some(n) = &m.name
            {
                sp.name = n.clone();
                sp.name_source = NameSource::Inferred;
            }
        }
        map.insert(raw.clone(), global);
    }
    map
}

// ---------- 整合 ----------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reconciliation {
    pub merges: Vec<(String, String)>,
    pub names: Vec<(String, String)>,
}

pub fn build_reconcile_request(prompts: &Prompts, model: &str, project: &Project) -> LlmRequest {
    let mut text = String::new();
    if !project.options.participants.is_empty() {
        text.push_str("與會者名單：\n");
        for p in &project.options.participants {
            text.push_str(&format!("- {p}\n"));
        }
        text.push('\n');
    }
    text.push_str("說話者清單（依發言量排序）：\n");
    let stats = project.speaker_stats();
    for (id, secs, count) in stats.iter().take(ROSTER_LIMIT) {
        let Some(sp) = project.speaker(id) else { continue };
        let chunks: std::collections::BTreeSet<usize> = project
            .segments
            .iter()
            .filter(|s| s.speaker.as_deref() == Some(id))
            .map(|s| s.chunk + 1)
            .collect();
        text.push_str(&format!(
            "\n## {id}｜目前名稱：{}｜發言 {count} 句、約 {:.0} 秒｜出現在第 {:?} 段\n聲音描述：{}\n發言樣本：\n",
            sp.name,
            secs,
            chunks,
            if sp.note.is_empty() { "（無）" } else { &sp.note }
        ));
        let samples: Vec<&str> = project
            .segments
            .iter()
            .filter(|s| s.speaker.as_deref() == Some(id))
            .map(|s| s.text.as_str())
            .filter(|t| t.chars().count() >= 6)
            .take(5)
            .collect();
        for s in samples {
            text.push_str(&format!("- 「{}」\n", s.chars().take(120).collect::<String>()));
        }
    }
    LlmRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::text(Role::System, prompts.reconcile.clone()),
            ChatMessage::text(Role::User, text),
        ],
        temperature: Some(0.0),
        max_tokens: Some(8000),
    }
}

pub fn parse_reconcile(raw: &str) -> Result<Reconciliation> {
    let json = extract_json(raw).ok_or_else(|| Error::BadModelOutput("找不到 JSON".into()))?;
    let v: Value = serde_json::from_str(json).map_err(|e| Error::BadModelOutput(e.to_string()))?;
    let id = |x: &Value| x.as_str().and_then(normalize_speaker_id);
    let merges = v["merges"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| Some((id(&m["from"])?, id(&m["into"])?)))
                .collect()
        })
        .unwrap_or_default();
    let names = v["names"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| Some((id(&m["id"])?, m["name"].as_str()?.trim().to_string())))
                .filter(|(_, n)| !n.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(Reconciliation { merges, names })
}

/// 套用整合結果；回傳實際合併數與命名數。使用者改過的名字不動。
pub fn apply_reconciliation(project: &mut Project, rec: &Reconciliation) -> (usize, usize) {
    let mut merged = 0;
    // 先解開鏈狀合併（A→B、B→C），避免併到已被刪掉的 ID
    let target: HashMap<String, String> = rec.merges.iter().cloned().collect();
    let resolve = |mut id: String, target: &HashMap<String, String>| {
        for _ in 0..10 {
            match target.get(&id) {
                Some(next) if *next != id => id = next.clone(),
                _ => break,
            }
        }
        id
    };
    let pairs: Vec<(String, String)> = rec
        .merges
        .iter()
        .map(|(f, _)| (f.clone(), resolve(f.clone(), &target)))
        .collect();
    for (from, into) in pairs {
        if project.merge_speakers(&from, &into) {
            merged += 1;
        }
    }
    let mut named = 0;
    for (id, name) in &rec.names {
        if let Some(sp) = project.speaker_mut(id)
            && sp.name_source != NameSource::User
        {
            sp.name = name.clone();
            sp.name_source = NameSource::Inferred;
            named += 1;
        }
    }
    (merged, named)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{JobOptions, Segment, Timing};

    #[test]
    fn parses_compact_and_object_forms() {
        let raw = r#"好的：```json
{"speakers":[{"id":"N1","name":"王經理","note":"女聲"},{"id":"S2","name":null,"note":""}],
 "a":[[0,"S2",0],[1,"n1","00:04.5"],{"i":2,"s":"S2","t":9.8},[3,"?",1]]}
```"#;
        let p = parse_response(raw).unwrap();
        assert_eq!(p.speakers[0].name.as_deref(), Some("王經理"));
        assert_eq!(p.speakers[1].name, None);
        assert_eq!(p.assignments.len(), 3);
        assert_eq!(
            p.assignments[1],
            Assignment {
                unit: 1,
                speaker: "N1".into(),
                t: Some(4.5)
            }
        );
        assert!(parse_response("抱歉，我無法處理").is_err());
    }

    #[test]
    fn resolve_fills_gaps() {
        let p = ParsedAttribution {
            speakers: vec![],
            assignments: vec![
                Assignment {
                    unit: 1,
                    speaker: "S1".into(),
                    t: Some(2.0),
                },
                Assignment {
                    unit: 3,
                    speaker: "N1".into(),
                    t: None,
                },
                Assignment {
                    unit: 9,
                    speaker: "S9".into(),
                    t: None,
                }, // 超出範圍
            ],
        };
        let (sp, times, covered) = resolve(&p, 5);
        assert_eq!(sp, vec!["S1", "S1", "S1", "N1", "N1"]);
        assert_eq!(times[1], Some(2.0));
        assert_eq!(covered, 2);
    }

    #[test]
    fn times_drop_outliers_and_interpolate() {
        let w = vec![1, 1, 1, 1];
        // 第 1 句的 500 是亂跳的值，應被剔除
        let t = fill_times(&w, &[Some(0.0), Some(500.0), Some(20.0), None], 40.0);
        assert_eq!(t[0], (0.0, 10.0));
        assert_eq!(t[1], (10.0, 20.0));
        assert_eq!(t[2], (20.0, 30.0));
        assert_eq!(t[3], (30.0, 40.0));
    }

    #[test]
    fn times_without_anchors_are_proportional() {
        let t = fill_times(&[1, 3], &[None, None], 8.0);
        assert_eq!(t, vec![(0.0, 2.0), (2.0, 8.0)]);
    }

    #[test]
    fn global_mapping_reuses_known_and_allocates_new() {
        let mut p = Project::new("x.mp3".into(), JobOptions::default());
        p.add_speaker(None, "男聲".into()); // S1
        let parsed = ParsedAttribution {
            speakers: vec![
                SpeakerInfo {
                    id: "S1".into(),
                    name: Some("陳主任".into()),
                    note: String::new(),
                },
                SpeakerInfo {
                    id: "N1".into(),
                    name: None,
                    note: "女聲".into(),
                },
            ],
            assignments: vec![],
        };
        let map = map_to_global(&mut p, &parsed, &["S1".into(), "N1".into(), "S7".into()]);
        assert_eq!(map["S1"], "S1");
        assert_eq!(map["N1"], "S2");
        assert_eq!(map["S7"], "S3"); // 模型亂編的 ID 當新的人
        assert_eq!(p.speaker("S1").unwrap().name, "陳主任");
        assert_eq!(p.speaker("S2").unwrap().note, "女聲");
    }

    #[test]
    fn reconciliation_resolves_chains_and_respects_user_names() {
        let mut p = Project::new("x.mp3".into(), JobOptions::default());
        for _ in 0..3 {
            p.add_speaker(None, String::new());
        }
        p.rename_speaker("S1", "我改的");
        p.segments = (1..=3)
            .map(|i| Segment {
                id: i,
                start: i as f64,
                end: i as f64 + 1.0,
                timing: Timing::Model,
                speaker: Some(format!("S{i}")),
                text: "x".into(),
                chunk: 0,
                edited: false,
            })
            .collect();
        let rec = parse_reconcile(
            r#"{"merges":[{"from":"S3","into":"S2"},{"from":"S2","into":"S1"}],"names":[{"id":"S1","name":"模型猜的"}]}"#,
        )
        .unwrap();
        let (merged, named) = apply_reconciliation(&mut p, &rec);
        assert_eq!((merged, named), (2, 0));
        assert_eq!(p.speakers.len(), 1);
        assert_eq!(p.speaker("S1").unwrap().name, "我改的");
        assert!(p.segments.iter().all(|s| s.speaker.as_deref() == Some("S1")));
    }
}
