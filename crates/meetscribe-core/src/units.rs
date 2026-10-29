//! 把一段逐字稿切成「句」，作為說話者分配與時間戳的最小單位。
//!
//! 說話者分配模型只回「第幾句是誰說的」，文字一律以語音辨識結果為準，
//! 所以逐字稿不會因為 LLM 改寫或偷懶而漏字。

use crate::model::TimedWord;
use crate::textutil::{normalize_for_compare, weight};

const PRIMARY_BREAKS: &[char] = &['。', '！', '？', '!', '?', '；', ';', '…', '\n'];
const SECONDARY_BREAKS: &[char] = &['，', ',', '、', '：', ':', ' '];

/// 切句：先依句末標點，太長的再依逗號／空白，最後硬切。`max_chars` 以字元計。
pub fn split_units(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(10);
    let mut out = Vec::new();
    for sentence in split_keep(text, |c, next| {
        PRIMARY_BREAKS.contains(&c) || (c == '.' && next.is_none_or(char::is_whitespace))
    }) {
        if sentence.chars().count() <= max_chars {
            push_trimmed(&mut out, &sentence);
            continue;
        }
        // 太長：在次要斷點上貪婪累積到接近上限
        let mut buf = String::new();
        for piece in split_keep(&sentence, |c, _| SECONDARY_BREAKS.contains(&c)) {
            if !buf.is_empty() && buf.chars().count() + piece.chars().count() > max_chars {
                push_trimmed(&mut out, &buf);
                buf.clear();
            }
            if piece.chars().count() > max_chars {
                for hard in hard_split(&piece, max_chars) {
                    push_trimmed(&mut out, &hard);
                }
            } else {
                buf.push_str(&piece);
            }
        }
        push_trimmed(&mut out, &buf);
    }
    out
}

fn push_trimmed(out: &mut Vec<String>, s: &str) {
    let t = s.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
}

/// 在 `is_break(c, 下一個字元)` 為真的字元之後切開，斷點字元留在前一段。
fn split_keep(text: &str, is_break: impl Fn(char, Option<char>) -> bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        cur.push(c);
        if is_break(c, chars.get(i + 1).copied()) {
            // 連續的句末符號（「？！」「……」）黏在一起
            if chars
                .get(i + 1)
                .is_some_and(|n| PRIMARY_BREAKS.contains(n) && *n != '\n')
            {
                continue;
            }
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn hard_split(s: &str, max_chars: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(max_chars).map(|c| c.iter().collect()).collect()
}

/// 解析 Nemotron 原生的說話者標籤 `<spk:0> 文字 <spk:1> 文字`。沒有標籤回傳空陣列。
pub fn parse_spk_tags(text: &str) -> Vec<(String, String)> {
    let re = regex::Regex::new(r"<spk:(\d+)>").expect("valid regex");
    let marks: Vec<(usize, usize, String)> = re
        .captures_iter(text)
        .map(|c| {
            let m = c.get(0).expect("whole match");
            (m.start(), m.end(), format!("S{}", &c[1]))
        })
        .collect();
    let mut out = Vec::new();
    for (k, (_, end, label)) in marks.iter().enumerate() {
        let next = marks.get(k + 1).map_or(text.len(), |m| m.0);
        let body = text[*end..next].trim();
        if !body.is_empty() {
            out.push((label.clone(), body.to_string()));
        }
    }
    out
}

/// 依字數比例把 [start, end] 分給各句。
pub fn estimate_times(units: &[String], start: f64, end: f64) -> Vec<(f64, f64)> {
    let total: usize = units.iter().map(|u| weight(u)).sum();
    let span = (end - start).max(0.0);
    let mut acc = 0usize;
    units
        .iter()
        .map(|u| {
            let s = start + span * acc as f64 / total.max(1) as f64;
            acc += weight(u);
            let e = start + span * acc as f64 / total.max(1) as f64;
            (s, e)
        })
        .collect()
}

/// 供應商有給字詞時間戳時，用累計字數對齊出每句的起訖。對不起來回傳 `None`。
pub fn times_from_words(units: &[String], words: &[TimedWord]) -> Option<Vec<(f64, f64)>> {
    if words.is_empty() {
        return None;
    }
    // 每個「可比對字元」對應到哪個 word
    let mut char_word: Vec<usize> = Vec::new();
    for (wi, w) in words.iter().enumerate() {
        for _ in normalize_for_compare(&w.text).chars() {
            char_word.push(wi);
        }
    }
    let unit_chars: usize = units.iter().map(|u| normalize_for_compare(u).chars().count()).sum();
    if char_word.is_empty() || unit_chars == 0 {
        return None;
    }
    // 字數或內容差太多代表不是同一份文字
    let ratio = char_word.len() as f64 / unit_chars as f64;
    let word_text: String = words.iter().map(|w| normalize_for_compare(&w.text)).collect();
    let unit_text: String = units.iter().map(|u| normalize_for_compare(u)).collect();
    if !(0.9..=1.1).contains(&ratio) || bigram_dice(&word_text, &unit_text) < 0.8 {
        return None;
    }
    let mut out = Vec::with_capacity(units.len());
    let mut pos = 0usize;
    for u in units {
        let n = normalize_for_compare(u).chars().count();
        let first = char_word[pos.min(char_word.len() - 1)];
        let last = char_word[(pos + n.max(1) - 1).min(char_word.len() - 1)];
        out.push((words[first].start, words[last].end.max(words[first].start)));
        pos += n;
    }
    Some(out)
}

/// 字元二元組的 Dice 相似度（0～1）。
fn bigram_dice(a: &str, b: &str) -> f64 {
    let grams = |s: &str| -> std::collections::HashMap<(char, char), usize> {
        let chars: Vec<char> = s.chars().collect();
        let mut m = std::collections::HashMap::new();
        for w in chars.windows(2) {
            *m.entry((w[0], w[1])).or_insert(0) += 1;
        }
        m
    };
    let (ga, gb) = (grams(a), grams(b));
    let (na, nb): (usize, usize) = (ga.values().sum(), gb.values().sum());
    if na + nb == 0 {
        return if a == b { 1.0 } else { 0.0 };
    }
    let common: usize = ga.iter().map(|(k, v)| (*v).min(*gb.get(k).unwrap_or(&0))).sum();
    2.0 * common as f64 / (na + nb) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_sentence_marks() {
        let u = split_units("大家好。今天討論預算！有問題嗎？好", 50);
        assert_eq!(u, vec!["大家好。", "今天討論預算！", "有問題嗎？", "好"]);
    }

    #[test]
    fn english_period_needs_space() {
        let u = split_units("Version 3.5 is out. Next topic", 50);
        assert_eq!(u, vec!["Version 3.5 is out.", "Next topic"]);
    }

    #[test]
    fn keeps_runs_of_marks_together() {
        let u = split_units("真的嗎？！對……好", 50);
        assert_eq!(u, vec!["真的嗎？！", "對……", "好"]);
    }

    #[test]
    fn long_text_without_punctuation_is_bounded() {
        let text = "這是一段沒有標點的很長的語音辨識結果".repeat(10);
        let u = split_units(&text, 30);
        assert!(u.iter().all(|s| s.chars().count() <= 30));
        assert_eq!(u.concat(), text);
    }

    #[test]
    fn secondary_breaks_used_first() {
        let u = split_units("第一部分講的是預算，第二部分講的是時程，第三部分講人力", 12);
        assert_eq!(
            u,
            vec!["第一部分講的是預算，", "第二部分講的是時程，", "第三部分講人力"]
        );
    }

    #[test]
    fn spk_tags() {
        let t = parse_spk_tags("<spk:0> 大家好。 <spk:1>你好<spk:0>");
        assert_eq!(t, vec![("S0".into(), "大家好。".into()), ("S1".into(), "你好".into())]);
        assert!(parse_spk_tags("沒有標籤").is_empty());
    }

    #[test]
    fn estimate_is_proportional_and_contiguous() {
        let units = vec!["ab".to_string(), "abcdef".to_string()];
        let t = estimate_times(&units, 10.0, 18.0);
        assert_eq!(t[0], (10.0, 12.0));
        assert_eq!(t[1], (12.0, 18.0));
    }

    #[test]
    fn word_alignment() {
        let words = vec![
            TimedWord {
                text: "hello".into(),
                start: 0.0,
                end: 0.5,
                speaker: None,
            },
            TimedWord {
                text: "world.".into(),
                start: 0.6,
                end: 1.0,
                speaker: None,
            },
            TimedWord {
                text: "next".into(),
                start: 2.0,
                end: 2.4,
                speaker: None,
            },
        ];
        let units = vec!["hello world.".to_string(), "next".to_string()];
        let t = times_from_words(&units, &words).unwrap();
        assert_eq!(t, vec![(0.0, 1.0), (2.0, 2.4)]);
        assert!(times_from_words(&["完全不同的一段很長很長的文字".into()], &words).is_none());
    }
}
