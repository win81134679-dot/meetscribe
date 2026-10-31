//! 每句逐字稿的起訖時間：跟隨播放、從某句播放、字幕都靠它。
//!
//! 2026-10-31 使用者回報「跟隨播放根本不準」。查一場 2.6 小時的真實會議，時間來源兩邊都不可靠：
//! - 說話者分配模型（Gemini 等）回的秒數常是假的：0、1、2、3…或每句 +0.2 秒的等差數列，整段 9 分鐘的字被擠在前 30 秒，
//!   最後一句佔掉剩下 8 分多。它們是遞增的，所以「最長非遞減子序列」擋不住。
//! - 供應商（DeepInfra Nemotron）以約 300 秒為一窗：沒有逐字時間的窗整窗回一段（300 秒一句）；最後一個不滿 300 秒的窗，
//!   逐字時間被拉長成像整窗那麼長，超出音訊結尾好幾分鐘。
//!
//! 所以時間一律先過 [`plausible_anchors`]／[`provider_times_ok`]，不可信就丟掉；沒有可信時間的地方用 [`Speech`]
//! 把字數分配到「有聲音的時間」上（靜音偵測），比平均分給整段準得多。

use crate::textutil::{normalize_for_compare, weight};

/// 說話速度上限（每秒幾個字母數字；中文約 4～6、英文字母約 12～18）。超過就是時間被擠扁了。
const MAX_CHARS_PER_SEC: f64 = 25.0;
/// 比這長的一句當成供應商沒切開的整窗，要重切。
pub const MAX_SEGMENT_SECS: f64 = 45.0;
/// 供應商約 300 秒一窗（真實會議量到 281～314 秒）：離上一窗開頭至少這麼久的段落才算新的一窗
/// （窗裡面的逐字段落、拉長的最後一窗裡的時間都不算）。
const MIN_WINDOW_GAP: f64 = 270.0;
/// 窗開頭的字找到之後，離最近的句子開頭不超過幾個字才採用（約 1 秒）。
const WINDOW_SNAP_CHARS: usize = 5;
/// 窗開頭的字在句子文字裡往前後找多遠（使用者改過字、句子文字與窗文字差一點時）。
const WINDOW_SEARCH_CHARS: usize = 40;
/// 時間對齊規則的版本。2：本檔的規則（2026-10-31）。3：供應商每窗的開頭當錨點（同日第二輪）。之前處理的專案會被重新對齊一次。
pub const TIMING_REV: u32 = 3;
/// 找「有聲音」用的靜音門檻：比切段用的（-30 dB、0.6 秒）敏感，句子之間的短停頓也算。
pub const SPEECH_NOISE_DB: i32 = -35;
pub const SPEECH_MIN_SILENCE: f64 = 0.35;

/// 一段音訊裡有聲音的區間（相對於段落開頭）。
#[derive(Clone, Debug, PartialEq)]
pub struct Speech {
    voiced: Vec<(f64, f64)>,
    duration: f64,
}

impl Speech {
    /// 整段都當有聲音（沒有靜音資訊時）。
    pub fn uniform(duration: f64) -> Self {
        Self { voiced: vec![(0.0, duration.max(0.0))], duration: duration.max(0.0) }
    }

    /// 由靜音區間（silencedetect 的輸出，相對秒）取補集。整段都是靜音時退回 [`Speech::uniform`]。
    pub fn from_silences(silences: &[(f64, f64)], duration: f64) -> Self {
        let mut s: Vec<(f64, f64)> = silences
            .iter()
            .map(|&(a, b)| (a.clamp(0.0, duration), b.clamp(0.0, duration)))
            .filter(|(a, b)| b > a)
            .collect();
        s.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut voiced = Vec::new();
        let mut t = 0.0;
        for (a, b) in s {
            if a > t {
                voiced.push((t, a));
            }
            t = t.max(b);
        }
        if duration > t {
            voiced.push((t, duration));
        }
        if voiced.iter().map(|(a, b)| b - a).sum::<f64>() < 0.5 {
            return Self::uniform(duration);
        }
        Self { voiced, duration }
    }

    /// 只看 [start, end) 這一段（相對秒），回傳以 start 為 0 的新 Speech。
    pub fn slice(&self, start: f64, end: f64) -> Self {
        let silences: Vec<(f64, f64)> = self
            .gaps()
            .into_iter()
            .map(|(a, b)| (a - start, b - start))
            .collect();
        Self::from_silences(&silences, (end - start).max(0.0))
    }

    fn gaps(&self) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        let mut t = 0.0;
        for &(a, b) in &self.voiced {
            if a > t {
                out.push((t, a));
            }
            t = b;
        }
        if self.duration > t {
            out.push((t, self.duration));
        }
        out
    }

    /// 0..t 之間有聲音的秒數。
    fn voiced_until(&self, t: f64) -> f64 {
        self.voiced.iter().map(|&(a, b)| (t.min(b) - a).max(0.0)).sum()
    }

    /// 有聲音的累計秒數 v 落在哪個時間點（[`Speech::voiced_until`] 的反函數）。剛好在一段聲音的結尾時
    /// 給下一段聲音的開頭：一句話從開口那一刻算起，不是從上一段話的尾巴。
    fn time_at(&self, v: f64) -> f64 {
        let mut acc = 0.0;
        for &(a, b) in &self.voiced {
            let len = b - a;
            if v < acc + len - 1e-9 {
                return a + (v - acc).max(0.0);
            }
            acc += len;
        }
        self.voiced.last().map_or(self.duration, |&(_, b)| b)
    }
}

/// 依字數把 [start, end] 分給各句，只算有聲音的時間：停頓不會被平均分掉，每句的開頭落在說話的地方。
pub fn spread(weights: &[usize], start: f64, end: f64, speech: &Speech) -> Vec<(f64, f64)> {
    let total: usize = weights.iter().sum::<usize>().max(1);
    let (v0, v1) = (speech.voiced_until(start), speech.voiced_until(end.max(start)));
    let span = v1 - v0;
    let at = |acc: usize| -> f64 {
        if span <= 1e-6 {
            return start + (end - start) * acc as f64 / total as f64;
        }
        speech.time_at(v0 + span * acc as f64 / total as f64).clamp(start, end.max(start))
    };
    let mut acc = 0usize;
    weights
        .iter()
        .map(|&w| {
            let s = at(acc);
            acc += w;
            (s, at(acc).max(s))
        })
        .collect()
}

/// 模型給的錨點：留下可信的（其餘設 None）。整組看起來是假的就全部不要。
/// 假的樣子：等差數列（0,1,2,3… 或每句 +0.2）、照它算說話快得不可能、遞減或超出段落。
pub fn plausible_anchors(weights: &[usize], anchors: &[Option<f64>], duration: f64) -> Vec<Option<f64>> {
    let n = weights.len().min(anchors.len());
    // 在段落內、遞增的那些
    let mut kept: Vec<(usize, f64)> = Vec::new();
    for (i, a) in anchors.iter().take(n).enumerate() {
        if let Some(t) = a.filter(|t| t.is_finite() && *t >= 0.0 && *t <= duration + 1.0)
            && kept.last().is_none_or(|&(_, prev)| t >= prev)
        {
            kept.push((i, t.min(duration)));
        }
    }
    let none = vec![None; anchors.len()];
    if kept.len() >= 5 {
        let diffs: Vec<f64> = kept.windows(2).map(|w| w[1].1 - w[0].1).collect();
        let first = diffs[0];
        let same = diffs.iter().filter(|d| (*d - first).abs() < 0.05).count();
        if first <= 2.0 && same as f64 >= diffs.len() as f64 * 0.8 {
            return none;
        }
    }
    // 由錨點推得的語速：前面的字擠在太短的時間裡＝時間被壓扁
    let prefix: Vec<usize> = std::iter::once(0)
        .chain(weights.iter().scan(0usize, |acc, w| {
            *acc += w;
            Some(*acc)
        }))
        .collect();
    if let Some(&(last_i, last_t)) = kept.last()
        && last_i > 0
        && prefix[last_i] as f64 > MAX_CHARS_PER_SEC * last_t.max(1.0)
    {
        return none;
    }
    // 兩兩之間太快的錨點剔除後面那個
    let mut out = vec![None; anchors.len()];
    let mut prev: Option<(usize, f64)> = None;
    for (i, t) in kept {
        let ok = prev.is_none_or(|(pi, pt)| {
            let chars = (prefix[i] - prefix[pi]) as f64;
            chars <= MAX_CHARS_PER_SEC * (t - pt).max(0.5)
        });
        if ok {
            out[i] = Some(t);
            prev = Some((i, t));
        }
    }
    out
}

/// 供應商給的每句起訖可信嗎：都在段落內、遞增、沒有超長的一句、語速合理。
pub fn provider_times_ok(texts: &[&str], times: &[(f64, f64)], duration: f64) -> bool {
    if texts.len() != times.len() || times.is_empty() {
        return false;
    }
    let mut prev = 0.0;
    for (text, &(s, e)) in texts.iter().zip(times) {
        if !(s.is_finite() && e.is_finite()) || s < prev - 1.0 || e > duration + 1.0 || e < s {
            return false;
        }
        if e - s > MAX_SEGMENT_SECS {
            return false;
        }
        if weight(text) as f64 > MAX_CHARS_PER_SEC * (e - s).max(0.5) {
            return false;
        }
        prev = s;
    }
    true
}

/// 供應商段落的開頭對到的那一句，當錨點。`units` 是同一份文字切成的句子；`provider` 是供應商的段落（文字、起、訖，相對段落開頭）。
///
/// 2026-10-31 第二輪（「跟隨播放還是跟不上」，一場音樂課）：窗裡沒有逐字時間時只能依字數估，講一段、拉一段琴的錄音
/// 會差到 30～60 秒；每窗的開頭是確定的時間，一律採用。窗裡面的段落開頭跟強制對齊比會差幾秒，但還是比字數估算準
/// （第 13 段：全用 11.4 秒、只用窗開頭 17.6 秒），所以也用——除了被拉長的最後一窗（段落結束超出音訊），那一窗只用開頭。
/// 哪些段落是「一窗的開頭」：第一段、整窗一段（超過 [`MAX_SEGMENT_SECS`]）的下一段、離上一窗開頭至少 [`MIN_WINDOW_GAP`] 的第一段。
pub fn provider_anchors(units: &[&str], provider: &[(&str, f64, f64)], duration: f64) -> Vec<Option<f64>> {
    let mut out = vec![None; units.len()];
    let norm = |s: &str| -> Vec<char> { normalize_for_compare(s).chars().collect() };
    let mut unit_at = Vec::with_capacity(units.len());
    let mut all: Vec<char> = Vec::new();
    for u in units {
        unit_at.push(all.len());
        all.extend(norm(u));
    }
    // 每段屬於哪一窗、是不是窗的開頭；每窗的段落最晚結束在哪
    let (mut last_window, mut after_block) = (f64::NEG_INFINITY, true);
    let mut window_of = Vec::with_capacity(provider.len());
    let mut starts_window = Vec::with_capacity(provider.len());
    let mut window_end: Vec<f64> = Vec::new();
    for &(_, s, e) in provider {
        // 超出段落結尾的不可能是新的一窗（是被拉長那一窗的後半）
        let is_window = s.is_finite() && s <= duration + 1.0 && (after_block || s - last_window >= MIN_WINDOW_GAP);
        after_block = e - s > MAX_SEGMENT_SECS;
        if is_window || window_end.is_empty() {
            if s.is_finite() {
                last_window = s;
            }
            window_end.push(f64::NEG_INFINITY);
        }
        let w = window_end.len() - 1;
        if e.is_finite() {
            window_end[w] = window_end[w].max(e);
        }
        window_of.push(w);
        starts_window.push(is_window);
    }
    let mut expected = 0usize;
    for (i, &(text, s, _)) in provider.iter().enumerate() {
        let t = norm(text);
        let stretched = window_end[window_of[i]] > duration + 1.0;
        if (starts_window[i] || !stretched) && s.is_finite() && s >= 0.0 {
            // 這窗的頭幾個字在句子文字裡的位置（使用者改過字會偏一點，在附近找）
            let key = &t[..t.len().min(6)];
            let found = (key.len() >= 2)
                .then(|| {
                    (expected.saturating_sub(WINDOW_SEARCH_CHARS)..=expected + WINDOW_SEARCH_CHARS)
                        .filter(|&p| all.get(p..p + key.len()) == Some(key))
                        .min_by_key(|&p| p.abs_diff(expected))
                })
                .flatten();
            if let Some(pos) = found
                && let Some(k) = (0..units.len()).min_by_key(|&k| unit_at[k].abs_diff(pos))
                && unit_at[k].abs_diff(pos) <= WINDOW_SNAP_CHARS
                && out[k].is_none()
            {
                out[k] = Some(s);
            }
        }
        expected += t.len();
    }
    out
}

/// 窗邊界錨點優先；模型的錨點只補在窗邊界之間、不跟它們衝突（時間倒退或語速不可能）的地方。
pub fn merge_anchors(weights: &[usize], windows: &[Option<f64>], model: &[Option<f64>]) -> Vec<Option<f64>> {
    let prefix: Vec<usize> = std::iter::once(0)
        .chain(weights.iter().scan(0usize, |acc, w| {
            *acc += w;
            Some(*acc)
        }))
        .collect();
    let chars = |a: usize, b: usize| (prefix[b.min(weights.len())] - prefix[a.min(weights.len())]) as f64;
    let mut out = windows.to_vec();
    out.resize(weights.len(), None);
    for (i, m) in model.iter().enumerate().take(out.len()) {
        let Some(t) = *m else { continue };
        if out[i].is_some() {
            continue;
        }
        let prev = out[..i].iter().enumerate().rev().find_map(|(j, a)| a.map(|v| (j, v)));
        let next = out[i + 1..].iter().enumerate().find_map(|(j, a)| a.map(|v| (i + 1 + j, v)));
        let ok_prev = prev.is_none_or(|(j, tp)| t >= tp && chars(j, i) <= MAX_CHARS_PER_SEC * (t - tp).max(0.5));
        let ok_next = next.is_none_or(|(j, tn)| tn >= t && chars(i, j) <= MAX_CHARS_PER_SEC * (tn - t).max(0.5));
        if ok_prev && ok_next {
            out[i] = Some(t);
        }
    }
    out
}

/// 有錨點用錨點，錨點之間與沒有錨點的地方依 [`spread`]。回傳每句 (開始, 結束)，相對於段落開頭。
pub fn fill(weights: &[usize], anchors: &[Option<f64>], duration: f64, speech: &Speech) -> Vec<(f64, f64)> {
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    let mut starts: Vec<Option<f64>> = anchors.iter().take(n).copied().collect();
    starts.resize(n, None);
    let first_anchor = starts.iter().position(Option::is_some);
    let mut out = vec![(0.0, 0.0); n];
    // 第一個錨點之前的句子：從段落開頭分到第一個錨點
    let mut i = 0;
    let mut seg_start = 0.0;
    if first_anchor != Some(0) {
        let j = first_anchor.unwrap_or(n);
        let end = first_anchor.map_or(duration, |j| starts[j].expect("anchor"));
        for (k, t) in spread(&weights[..j], 0.0, end, speech).into_iter().enumerate() {
            out[k] = t;
        }
        i = j;
        seg_start = end;
    }
    while i < n {
        let s = starts[i].unwrap_or(seg_start);
        let mut j = i + 1;
        while j < n && starts[j].is_none() {
            j += 1;
        }
        let end = if j < n { starts[j].expect("anchor") } else { duration };
        for (k, t) in spread(&weights[i..j], s, end.max(s), speech).into_iter().enumerate() {
            out[i + k] = t;
        }
        // 錨點就是那句的開頭（spread 可能把它往後挪到有聲音處）
        out[i].0 = s;
        seg_start = end;
        i = j;
    }
    // 首尾相接：每句結束＝下一句開始
    for k in 0..n.saturating_sub(1) {
        out[k].1 = out[k + 1].0.max(out[k].0);
    }
    out[n - 1].1 = duration.max(out[n - 1].0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(n: usize, each: usize) -> Vec<usize> {
        vec![each; n]
    }

    /// 文字進度 q 到的時候，時間走到段落的幾成。
    fn time_frac_at_text(weights: &[usize], times: &[(f64, f64)], duration: f64, q: f64) -> f64 {
        let total: usize = weights.iter().sum();
        let mut acc = 0;
        for (k, wt) in weights.iter().enumerate() {
            if acc as f64 >= q * total as f64 {
                return times[k].0 / duration;
            }
            acc += wt;
        }
        1.0
    }

    #[test]
    fn fake_grid_anchors_from_the_real_meeting_are_rejected() {
        // 真實會議第 8 段：563 秒、165 句，模型回 0, 0.2, 0.4 … 32.8
        let weights = w(165, 15);
        let anchors: Vec<Option<f64>> = (0..165).map(|i| Some(i as f64 * 0.2)).collect();
        assert!(plausible_anchors(&weights, &anchors, 563.0).iter().all(Option::is_none));
        let t = fill(&weights, &plausible_anchors(&weights, &anchors, 563.0), 563.0, &Speech::uniform(563.0));
        let f = time_frac_at_text(&weights, &t, 563.0, 0.95);
        assert!(f > 0.85, "95% 的字應該在段落後段，實際在 {:.0}%", f * 100.0);
        // 每秒 1 格的版本（第 2 段：0,1,2…28 秒、309 秒長）
        let anchors: Vec<Option<f64>> = (0..29).map(|i| Some(i as f64)).collect();
        assert!(plausible_anchors(&w(29, 18), &anchors, 309.0).iter().all(Option::is_none));
    }

    #[test]
    fn compressed_anchors_are_rejected_even_without_a_grid() {
        // 不等差但全部擠在前 40 秒：等於每秒 60 字
        let weights = w(100, 24);
        let anchors: Vec<Option<f64>> = (0..100).map(|i| Some(i as f64 * 0.4 + (i % 3) as f64 * 0.05)).collect();
        assert!(plausible_anchors(&weights, &anchors, 500.0).iter().all(Option::is_none));
    }

    #[test]
    fn real_looking_anchors_are_kept() {
        let weights = vec![20, 35, 12, 40, 18, 25];
        let anchors = vec![Some(0.0), Some(4.3), Some(11.0), Some(13.2), Some(22.5), Some(26.0)];
        assert_eq!(plausible_anchors(&weights, &anchors, 40.0), anchors);
        // 一個亂跳的被剔除，其他留下
        let bad = vec![Some(0.0), Some(4.3), Some(4.4), Some(13.2), Some(22.5), Some(26.0)];
        let kept = plausible_anchors(&weights, &bad, 40.0);
        assert_eq!(kept[2], None);
        assert_eq!(kept[3], Some(13.2));
    }

    #[test]
    fn provider_blocks_and_stretched_tail_are_not_trusted() {
        // 300 秒一整窗
        assert!(!provider_times_ok(&["很長的一段"; 2], &[(0.0, 298.0), (298.0, 580.0)], 653.0));
        // 最後一窗被拉長：超出 653 秒
        assert!(!provider_times_ok(&["好", "的"], &[(600.0, 601.0), (858.0, 860.0)], 653.0));
        assert!(provider_times_ok(&["大家好", "今天開會"], &[(0.5, 1.6), (2.0, 3.4)], 10.0));
    }

    #[test]
    fn spread_puts_sentences_on_speech_not_on_silence() {
        // 0–10 秒說話、10–50 秒安靜、50–60 秒說話；兩句一樣長
        let speech = Speech::from_silences(&[(10.0, 50.0)], 60.0);
        let t = spread(&[10, 10], 0.0, 60.0, &speech);
        assert!((t[0].0 - 0.0).abs() < 1e-6);
        assert!((t[1].0 - 50.0).abs() < 1e-6, "第二句從 50 秒開始說，實際 {:?}", t[1]);
        // 平均分的話第二句會在 30 秒（安靜中）
    }

    #[test]
    fn fill_keeps_anchors_and_spreads_between_them() {
        let speech = Speech::uniform(100.0);
        let t = fill(&[10, 10, 10, 10], &[None, Some(40.0), None, None], 100.0, &speech);
        assert!((t[0].0 - 0.0).abs() < 1e-6 && (t[1].0 - 40.0).abs() < 1e-6);
        assert!((t[2].0 - 60.0).abs() < 1e-6 && (t[3].0 - 80.0).abs() < 1e-6);
        assert!(t.windows(2).all(|p| (p[0].1 - p[1].0).abs() < 1e-9), "首尾相接");
        assert!((t[3].1 - 100.0).abs() < 1e-6);
    }

    #[test]
    fn window_starts_become_anchors() {
        // 真實會議第 9 段：兩窗（0–298、298–568），第二窗開頭那句被字數估算放到 363 秒
        let units = ["甲甲甲。", "乙乙。", "丙丙丙丙。", "丁丁。"];
        let windows = [("甲甲甲。乙乙。", 0.0, 298.0), ("丙丙丙丙。丁丁。", 298.0, 568.0)];
        assert_eq!(provider_anchors(&units, &windows, 568.0), vec![Some(0.0), None, Some(298.0), None]);
        // 窗邊界落在句子中間、離下一句開頭 2 個字：下一句從窗開頭算（差不到 1 秒）
        let units = ["甲甲甲乙乙。", "丙丙丙丙。"];
        let windows = [("甲甲甲", 0.0, 298.0), ("乙乙。丙丙丙丙。", 298.0, 400.0)];
        assert_eq!(provider_anchors(&units, &windows, 400.0), vec![Some(0.0), Some(298.0)]);
        // 離最近的句子開頭太遠就不給
        let units = ["甲甲甲乙乙乙乙乙乙乙乙乙乙。", "丙丙丙丙。"];
        let windows = [("甲甲甲", 0.0, 298.0), ("乙乙乙乙乙乙乙乙乙乙。丙丙丙丙。", 298.0, 400.0)];
        assert_eq!(provider_anchors(&units, &windows, 400.0), vec![Some(0.0), None]);
        // 文字對不上（使用者大改過）就不給
        let units = ["完全不同的句子一。", "完全不同的句子二。"];
        assert!(provider_anchors(&units, &[("甲甲甲甲甲甲甲甲甲甲", 0.0, 298.0), ("乙", 298.0, 300.0)], 300.0)[1].is_none());
    }

    #[test]
    fn stretched_last_window_only_gives_its_start() {
        // 第 17 段（370 秒）：第二窗從 290 秒開始，但逐字時間被拉長到 578 秒；裡面的 300、325… 都不可信
        let units = ["甲甲甲甲。", "乙乙。", "丙丙。", "丁丁。"];
        let windows = [("甲甲甲甲。", 0.0, 289.0), ("乙乙。", 290.0, 297.0), ("丙丙。", 300.0, 303.0), ("丁丁。", 569.0, 578.0)];
        assert_eq!(provider_anchors(&units, &windows, 370.0), vec![Some(0.0), Some(290.0), None, None]);
        // 第 13 段（674 秒）：第一窗沒被拉長，裡面的逐字段落（15、242 秒）也用；242 秒不是新的一窗，第二窗 294 秒開始
        let windows = [("甲甲甲甲。", 0.0, 7.0), ("乙乙。", 15.0, 17.0), ("丙丙。", 242.0, 247.0), ("丁丁。", 294.0, 577.0)];
        assert_eq!(provider_anchors(&units, &windows, 674.0), vec![Some(0.0), Some(15.0), Some(242.0), Some(294.0)]);
        // 同樣的段落但 300 秒就結束：第二窗超出結尾＝被拉長，第一窗照用
        let windows = [("甲甲甲甲。", 0.0, 7.0), ("乙乙。", 15.0, 17.0), ("丙丙。", 294.0, 296.0), ("丁丁。", 310.0, 577.0)];
        assert_eq!(provider_anchors(&units, &windows, 330.0), vec![Some(0.0), Some(15.0), Some(294.0), None]);
    }

    #[test]
    fn model_anchors_fill_in_between_window_starts() {
        let weights = [10, 10, 10, 10, 10];
        let windows = [Some(0.0), None, Some(100.0), None, None];
        let model = [None, Some(50.0), Some(30.0), Some(90.0), Some(140.0)];
        // 第 2 句以窗邊界為準；第 3 句的 90 秒早於第 2 句的 100 秒，不要
        assert_eq!(merge_anchors(&weights, &windows, &model), vec![Some(0.0), Some(50.0), Some(100.0), None, Some(140.0)]);
    }

    #[test]
    fn slice_rebases_silences() {
        let s = Speech::from_silences(&[(5.0, 8.0), (20.0, 30.0)], 40.0);
        let part = s.slice(10.0, 40.0);
        let t = spread(&[1, 1], 0.0, 30.0, &part);
        // 10–20 有聲音、20–30 安靜、30–40 有聲音 → 相對 0–10、20–30
        assert!((t[1].0 - 20.0).abs() < 1e-6, "{t:?}");
    }
}
