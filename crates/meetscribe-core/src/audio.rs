//! ffmpeg：探測長度、轉成 16 kHz mono、找靜音、依靜音切段、剪樣本。

use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// 正規化後的格式。16 kHz mono 是 Nemotron 的原生取樣率；32 kbps MP3 一小時約 14 MB。
pub const NORMALIZED_EXT: &str = "mp3";
pub const NORMALIZED_FORMAT: &str = "mp3";
/// 正規化後的取樣率。
pub const TARGET_RATE: u32 = 16_000;

/// 處理流程用的音訊工具：桌面叫 ffmpeg；Android 沒有 ffmpeg，用內建的解碼與編碼（`native-audio` feature）。
#[derive(Clone, Debug)]
pub enum AudioTool {
    Ffmpeg(Ffmpeg),
    #[cfg(feature = "native-audio")]
    Native(crate::audio_native::NativeAudio),
}

impl AudioTool {
    /// Android 一律用內建後端；其他平台照 [`Ffmpeg::locate`] 找 ffmpeg。
    pub fn locate(custom: &str) -> Result<Self> {
        #[cfg(all(target_os = "android", feature = "native-audio"))]
        {
            let _ = custom;
            Ok(Self::Native(crate::audio_native::NativeAudio))
        }
        #[cfg(not(all(target_os = "android", feature = "native-audio")))]
        {
            Ffmpeg::locate(custom).map(Self::Ffmpeg)
        }
    }

    pub async fn probe_duration(&self, input: &Path) -> Result<f64> {
        match self {
            Self::Ffmpeg(f) => f.probe_duration(input).await,
            #[cfg(feature = "native-audio")]
            Self::Native(n) => n.probe_duration(input).await,
        }
    }

    pub async fn normalize(
        &self,
        input: &Path,
        output: &Path,
        total_secs: f64,
        progress: &(dyn Fn(f32) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<()> {
        match self {
            Self::Ffmpeg(f) => f.normalize(input, output, total_secs, progress, cancel).await,
            #[cfg(feature = "native-audio")]
            Self::Native(n) => n.normalize(input, output, total_secs, progress, cancel).await,
        }
    }

    pub async fn detect_silences(
        &self,
        input: &Path,
        noise_db: i32,
        min_secs: f64,
        cancel: &CancellationToken,
    ) -> Result<Vec<(f64, f64)>> {
        match self {
            Self::Ffmpeg(f) => f.detect_silences(input, noise_db, min_secs, cancel).await,
            #[cfg(feature = "native-audio")]
            Self::Native(n) => n.detect_silences(input, noise_db, min_secs, cancel).await,
        }
    }

    pub async fn extract(&self, input: &Path, start: f64, end: f64, output: &Path) -> Result<()> {
        match self {
            Self::Ffmpeg(f) => f.extract(input, start, end, output).await,
            #[cfg(feature = "native-audio")]
            Self::Native(n) => n.extract(input, start, end, output).await,
        }
    }

    /// 播放用的高品質副本放進 `dir`，回傳檔名（2026-10-31 使用者：「音訊格式是否可以改成 aac 高品質避免品質變差」）。
    /// 送轉錄的 16 kHz 版本是給模型聽的（Nemotron 本來就以 16 kHz 處理），人聽會覺得悶；播放、核對改聽這份。
    /// 電腦：ffmpeg 轉 AAC 128 kbps 單聲道、保留原取樣率（`playback.m4a`）。Android：沒有 AAC 編碼器，直接保留原始檔（不重壓、零損失）。
    pub async fn playback_copy(&self, input: &Path, dir: &Path, cancel: &CancellationToken) -> Result<String> {
        match self {
            Self::Ffmpeg(f) => f.playback_copy(input, dir, cancel).await,
            #[cfg(feature = "native-audio")]
            Self::Native(_) => {
                let ext = input
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .filter(|e| !e.is_empty() && e.len() <= 5)
                    .unwrap_or_else(|| "audio".into());
                let name = format!("{PLAYBACK_STEM}.{ext}");
                tokio::fs::copy(input, dir.join(&name)).await?;
                Ok(name)
            }
        }
    }
}

/// 播放用副本的檔名（不含副檔名）。
pub const PLAYBACK_STEM: &str = "playback";

#[derive(Clone, Debug)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Ffmpeg {
    /// 尋找順序：設定值 → 程式旁邊 → PATH → winget 安裝位置。
    pub fn locate(custom: &str) -> Result<Self> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        let custom = custom.trim().trim_matches('"');
        if !custom.is_empty() {
            let p = PathBuf::from(custom);
            if p.is_dir() {
                candidates.push(p.join(exe("ffmpeg")));
                candidates.push(p.join("bin").join(exe("ffmpeg")));
            } else {
                candidates.push(p);
            }
        }
        if let Ok(me) = std::env::current_exe()
            && let Some(dir) = me.parent()
        {
            candidates.push(dir.join(exe("ffmpeg")));
            candidates.push(dir.join("ffmpeg").join("bin").join(exe("ffmpeg")));
        }
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|d| d.join(exe("ffmpeg"))));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let pkgs = PathBuf::from(local).join("Microsoft").join("WinGet").join("Packages");
            if let Ok(rd) = std::fs::read_dir(&pkgs) {
                for e in rd.flatten() {
                    if e.file_name().to_string_lossy().contains("FFmpeg")
                        && let Ok(inner) = std::fs::read_dir(e.path())
                    {
                        for i in inner.flatten() {
                            candidates.push(i.path().join("bin").join(exe("ffmpeg")));
                        }
                    }
                }
            }
        }
        for c in candidates {
            if c.is_file() {
                let probe = c.with_file_name(exe("ffprobe"));
                if probe.is_file() {
                    return Ok(Self {
                        ffmpeg: c,
                        ffprobe: probe,
                    });
                }
            }
        }
        Err(Error::FfmpegMissing(
            "請安裝 ffmpeg（winget install Gyan.FFmpeg），或在設定指定 ffmpeg.exe 的位置".into(),
        ))
    }

    pub async fn version(&self) -> Result<String> {
        let out = command(&self.ffmpeg).arg("-version").output().await?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text.lines().next().unwrap_or("").to_string())
    }

    /// 音訊長度（秒）。
    pub async fn probe_duration(&self, input: &Path) -> Result<f64> {
        let out = command(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=nw=1:nk=1",
            ])
            .arg(input)
            .output()
            .await?;
        if !out.status.success() {
            return Err(Error::Ffmpeg(format!(
                "無法讀取「{}」：{}",
                input.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let s = String::from_utf8_lossy(&out.stdout);
        s.trim()
            .parse::<f64>()
            .ok()
            .filter(|d| d.is_finite() && *d > 0.0)
            .ok_or_else(|| Error::Ffmpeg(format!("「{}」沒有可用的音軌", input.display())))
    }

    /// 轉成 16 kHz mono MP3 64 kbps（送轉錄用；1.0 是 32 kbps，壓縮痕跡比較明顯）。`progress` 收到 0..=1。
    pub async fn normalize(
        &self,
        input: &Path,
        output: &Path,
        total_secs: f64,
        progress: &(dyn Fn(f32) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<()> {
        let mut cmd = command(&self.ffmpeg);
        cmd.args(["-hide_banner", "-nostdin", "-y", "-i"])
            .arg(input)
            .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "libmp3lame", "-b:a", "64k"])
            .args(["-progress", "pipe:1", "-nostats"])
            .arg(output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let err_task = tokio::spawn(read_tail(stderr));
        let mut lines = BufReader::new(stdout).lines();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    return Err(Error::Cancelled);
                }
                line = lines.next_line() => {
                    match line? {
                        Some(l) => {
                            if let Some(us) = l.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok())
                                && total_secs > 0.0
                            {
                                progress((us / 1e6 / total_secs).clamp(0.0, 1.0) as f32);
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        let status = child.wait().await?;
        let tail = err_task.await.unwrap_or_default();
        if !status.success() {
            return Err(Error::Ffmpeg(format!("轉檔失敗：{tail}")));
        }
        progress(1.0);
        Ok(())
    }

    /// 回傳靜音區間 `(開始, 結束)`。
    pub async fn detect_silences(
        &self,
        input: &Path,
        noise_db: i32,
        min_secs: f64,
        cancel: &CancellationToken,
    ) -> Result<Vec<(f64, f64)>> {
        let filter = format!("silencedetect=noise={noise_db}dB:d={min_secs}");
        let mut cmd = command(&self.ffmpeg);
        cmd.args(["-hide_banner", "-nostdin", "-i"])
            .arg(input)
            .args(["-af", &filter, "-f", "null", "-"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let mut stderr = child.stderr.take().expect("piped");
        let mut buf = String::new();
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return Err(Error::Cancelled);
            }
            r = stderr.read_to_string(&mut buf) => { r?; }
        }
        let status = child.wait().await?;
        if !status.success() {
            return Err(Error::Ffmpeg(format!("靜音偵測失敗：{}", last_lines(&buf, 5))));
        }
        Ok(parse_silences(&buf))
    }

    /// 剪出 [start, end) 一段（MP3 直接複製，不重新編碼）。
    /// 見 [`AudioTool::playback_copy`]。
    pub async fn playback_copy(&self, input: &Path, dir: &Path, cancel: &CancellationToken) -> Result<String> {
        let name = format!("{PLAYBACK_STEM}.m4a");
        let out = command(&self.ffmpeg)
            .args(["-hide_banner", "-nostdin", "-y", "-i"])
            .arg(input)
            .args(["-vn", "-ac", "1", "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart"])
            .arg(dir.join(&name))
            .kill_on_drop(true)
            .output();
        let out = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            o = out => o?,
        };
        if !out.status.success() {
            let _ = std::fs::remove_file(dir.join(&name));
            return Err(Error::Ffmpeg(format!(
                "轉播放用音訊失敗：{}",
                last_lines(&String::from_utf8_lossy(&out.stderr), 5)
            )));
        }
        Ok(name)
    }

    pub async fn extract(&self, input: &Path, start: f64, end: f64, output: &Path) -> Result<()> {
        let out = command(&self.ffmpeg)
            .args(["-hide_banner", "-nostdin", "-y", "-ss", &format!("{start:.3}"), "-i"])
            .arg(input)
            .args(["-t", &format!("{:.3}", (end - start).max(0.1)), "-c", "copy"])
            .arg(output)
            .output()
            .await?;
        if !out.status.success() {
            return Err(Error::Ffmpeg(format!(
                "剪輯失敗：{}",
                last_lines(&String::from_utf8_lossy(&out.stderr), 5)
            )));
        }
        Ok(())
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// GUI 程式叫 ffmpeg 時不要閃黑窗。
fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.kill_on_drop(true);
    c
}

async fn read_tail(mut r: impl tokio::io::AsyncRead + Unpin) -> String {
    let mut buf = String::new();
    let _ = r.read_to_string(&mut buf).await;
    last_lines(&buf, 6)
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

/// 解析 silencedetect 的 stderr。
pub fn parse_silences(stderr: &str) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut pending: Option<f64> = None;
    for line in stderr.lines() {
        if let Some(v) = value_after(line, "silence_start:") {
            pending = Some(v.max(0.0));
        } else if let Some(v) = value_after(line, "silence_end:")
            && let Some(s) = pending.take()
        {
            out.push((s, v));
        }
    }
    out
}

fn value_after(line: &str, key: &str) -> Option<f64> {
    let rest = &line[line.find(key)? + key.len()..];
    rest.split_whitespace().next()?.parse().ok()
}

/// 決定切點：每段目標 `target` 秒，在理想切點前後 `window` 秒內挑最長的靜音、
/// 同長時挑離理想點近的；找不到就硬切。最後一段可以比目標長最多 `window`。
pub fn plan_chunks(duration: f64, silences: &[(f64, f64)], target: f64, window: f64) -> Vec<(f64, f64)> {
    let mut cuts = Vec::new();
    let mut pos = 0.0;
    while duration - pos > target + window {
        let ideal = pos + target;
        let best = silences
            .iter()
            .map(|&(s, e)| ((s + e) / 2.0, e - s))
            .filter(|&(mid, _)| (mid - ideal).abs() <= window && mid > pos + target * 0.5)
            .max_by(|a, b| {
                a.1.total_cmp(&b.1)
                    .then_with(|| (b.0 - ideal).abs().total_cmp(&(a.0 - ideal).abs()))
            });
        let cut = best.map_or(ideal, |(mid, _)| mid);
        cuts.push(cut);
        pos = cut;
    }
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut start = 0.0;
    for c in cuts {
        out.push((start, c));
        start = c;
    }
    out.push((start, duration));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_silencedetect_output() {
        let log = "[silencedetect @ 0x1] silence_start: 10.5\n\
                   [silencedetect @ 0x1] silence_end: 12.25 | silence_duration: 1.75\n\
                   [silencedetect @ 0x1] silence_start: -0.01\n\
                   [silencedetect @ 0x1] silence_end: 0.8 | silence_duration: 0.81\n";
        assert_eq!(parse_silences(log), vec![(10.5, 12.25), (0.0, 0.8)]);
    }

    #[test]
    fn short_audio_is_one_chunk() {
        assert_eq!(plan_chunks(600.0, &[], 1200.0, 90.0), vec![(0.0, 600.0)]);
        // 比目標長一點點也不切
        assert_eq!(plan_chunks(1250.0, &[], 1200.0, 90.0), vec![(0.0, 1250.0)]);
    }

    #[test]
    fn cuts_at_longest_silence_in_window() {
        let silences = [(1150.0, 1151.0), (1230.0, 1234.0), (1500.0, 1510.0)];
        let chunks = plan_chunks(3000.0, &silences, 1200.0, 90.0);
        assert_eq!(chunks[0], (0.0, 1232.0));
        assert_eq!(chunks.last().unwrap().1, 3000.0);
        // 連續、無縫
        for w in chunks.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
    }

    #[test]
    fn hard_cut_without_silence() {
        let chunks = plan_chunks(3650.0, &[], 1200.0, 90.0);
        assert_eq!(chunks, vec![(0.0, 1200.0), (1200.0, 2400.0), (2400.0, 3650.0)]);
    }

    #[test]
    fn long_meeting_chunks_stay_bounded() {
        // 8 小時、每 7 秒一個短停頓
        let silences: Vec<(f64, f64)> = (1..4000).map(|i| (i as f64 * 7.0, i as f64 * 7.0 + 0.6)).collect();
        let chunks = plan_chunks(8.0 * 3600.0, &silences, 1200.0, 90.0);
        assert!(chunks.iter().all(|(s, e)| e - s <= 1200.0 + 2.0 * 90.0));
        assert_eq!(chunks.len(), 24);
    }
}
