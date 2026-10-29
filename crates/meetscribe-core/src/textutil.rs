//! 時間格式、從模型輸出撈 JSON 這類小工具。

/// `01:02:03`；不到一小時時 `02:03`。
pub fn fmt_clock(sec: f64) -> String {
    let total = sec.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 一律帶小時：`00:02:03`。
pub fn fmt_hms(sec: f64) -> String {
    let total = sec.max(0.0).round() as u64;
    format!("{:02}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60)
}

/// SRT 用 `00:01:02,345`，VTT 用 `00:01:02.345`。
pub fn fmt_subtitle(sec: f64, decimal: char) -> String {
    let ms_total = (sec.max(0.0) * 1000.0).round() as u64;
    let (h, rem) = (ms_total / 3_600_000, ms_total % 3_600_000);
    let (m, rem) = (rem / 60_000, rem % 60_000);
    let (s, ms) = (rem / 1000, rem % 1000);
    format!("{h:02}:{m:02}:{s:02}{decimal}{ms:03}")
}

/// 時長的口語寫法：`1 小時 5 分`、`12 分 30 秒`。
pub fn fmt_duration_zh(sec: f64) -> String {
    let total = sec.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    match (h, m) {
        (0, 0) => format!("{s} 秒"),
        (0, _) => format!("{m} 分 {s} 秒"),
        _ => format!("{h} 小時 {m} 分"),
    }
}

/// 從模型輸出取出 JSON：去掉 ```json 圍欄與前後說明文字，取第一個 `{`／`[` 到最後一個對應括號。
pub fn extract_json(raw: &str) -> Option<&str> {
    let s = raw.trim();
    let start = s.find(['{', '['])?;
    let open = s.as_bytes()[start];
    let close = if open == b'{' { '}' } else { ']' };
    let end = s.rfind(close)?;
    (end > start).then(|| &s[start..=end])
}

/// 去掉空白與標點，只留可比對的字元（比對文字是否被改寫或漏掉用）。
pub fn normalize_for_compare(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 可比對字元數（用來把時間依字數比例分配）。
pub fn weight(s: &str) -> usize {
    s.chars().filter(|c| c.is_alphanumeric()).count().max(1)
}

/// 說話者 ID 正規化：`s1`、`Speaker 1`、`1`、`spk:1` → `S1`；`n2`、`New 2` → `N2`。
pub fn normalize_speaker_id(raw: &str) -> Option<String> {
    let t = raw.trim().to_ascii_uppercase();
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    let prefix = if t.starts_with('N') { 'N' } else { 'S' };
    Some(format!("{prefix}{n}"))
}

/// 檔名不可用的字元換掉。
pub fn safe_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() { "meeting".into() } else { trimmed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(fmt_clock(65.4), "01:05");
        assert_eq!(fmt_clock(3725.0), "01:02:05");
        assert_eq!(fmt_subtitle(62.3456, ','), "00:01:02,346");
        assert_eq!(fmt_subtitle(3600.0, '.'), "01:00:00.000");
        assert_eq!(fmt_duration_zh(3900.0), "1 小時 5 分");
    }

    #[test]
    fn json_extraction() {
        assert_eq!(extract_json("```json\n{\"a\":1}\n```"), Some("{\"a\":1}"));
        assert_eq!(extract_json("好的，結果如下：[1,2] 完成"), Some("[1,2]"));
        assert_eq!(extract_json("沒有 JSON"), None);
    }

    #[test]
    fn speaker_ids() {
        assert_eq!(normalize_speaker_id("speaker 3").as_deref(), Some("S3"));
        assert_eq!(normalize_speaker_id("<spk:0>").as_deref(), Some("S0"));
        assert_eq!(normalize_speaker_id("n2").as_deref(), Some("N2"));
        assert_eq!(normalize_speaker_id("?"), None);
    }

    #[test]
    fn filenames() {
        assert_eq!(safe_filename("週會: 10/30?"), "週會_ 10_30_");
    }
}
