//! 不靠 ffmpeg 的音訊後端（Android 用）：symphonia 解碼、自己重取樣成 16 kHz mono、LAME 編 64 kbps MP3。
//!
//! 介面與 [`crate::audio::Ffmpeg`] 相同，靜音偵測的判定照 ffmpeg `silencedetect`：
//! 每個取樣的振幅都低於門檻、而且連續超過指定秒數才算一段靜音。

use crate::audio::TARGET_RATE;
use crate::{Error, Result};
use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, MonoPcm, Quality};
use std::io::Write;
use std::path::{Path, PathBuf};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default)]
pub struct NativeAudio;

impl NativeAudio {
    pub fn version(&self) -> String {
        "內建音訊處理（symphonia＋LAME）".into()
    }

    pub async fn probe_duration(&self, input: &Path) -> Result<f64> {
        let input = input.to_path_buf();
        blocking(move || {
            let mut src = Source::open(&input)?;
            if let Some(d) = src.declared_duration() {
                return Ok(d);
            }
            // 沒有標頭資訊（例如 CBR MP3 沒有 Xing）就整個解一次
            let mut frames = 0u64;
            let mut rate = 0u32;
            src.decode(&CancellationToken::new(), |pcm, r| {
                frames += pcm.len() as u64;
                rate = r;
                true
            })?;
            if rate == 0 || frames == 0 {
                return Err(Error::Ffmpeg(format!("「{}」沒有可用的音軌", input.display())));
            }
            Ok(frames as f64 / rate as f64)
        })
        .await
    }

    /// 轉成 16 kHz mono 64 kbps MP3。`progress` 收到 0..=1。
    pub async fn normalize(
        &self,
        input: &Path,
        output: &Path,
        total_secs: f64,
        progress: &(dyn Fn(f32) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<()> {
        // progress 不是 'static，用 channel 把進度帶回這個 task 回報
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<f32>();
        let (input, output, cancel2) = (input.to_path_buf(), output.to_path_buf(), cancel.clone());
        let job = tokio::task::spawn_blocking(move || {
            let mut src = Source::open(&input)?;
            let mut enc = Mp3Writer::create(&output)?;
            let mut rs: Option<Resampler> = None;
            let mut seconds = 0.0f64;
            let mut last = 0.0f64;
            let mut out = Vec::new();
            let mut err = None;
            src.decode(&cancel2, |pcm, rate| {
                let r = rs.get_or_insert_with(|| Resampler::new(rate));
                out.clear();
                r.push(pcm, &mut out);
                if let Err(e) = enc.write(&out) {
                    err = Some(e);
                    return false;
                }
                seconds += pcm.len() as f64 / rate as f64;
                if total_secs > 0.0 && seconds - last >= 5.0 {
                    last = seconds;
                    let _ = tx.send((seconds / total_secs).clamp(0.0, 1.0) as f32);
                }
                true
            })?;
            if let Some(e) = err {
                return Err(e);
            }
            if let Some(mut r) = rs {
                out.clear();
                r.finish(&mut out);
                enc.write(&out)?;
            }
            enc.finish()
        });
        tokio::pin!(job);
        loop {
            tokio::select! {
                Some(f) = rx.recv() => progress(f),
                r = &mut job => {
                    r.map_err(|e| Error::Ffmpeg(format!("轉檔中斷：{e}")))??;
                    break;
                }
            }
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
        let (input, cancel) = (input.to_path_buf(), cancel.clone());
        blocking(move || {
            let mut src = Source::open(&input)?;
            let mut det: Option<SilenceDetector> = None;
            src.decode(&cancel, |pcm, rate| {
                det.get_or_insert_with(|| SilenceDetector::new(rate, noise_db, min_secs))
                    .push(pcm);
                true
            })?;
            Ok(det.map(SilenceDetector::finish).unwrap_or_default())
        })
        .await
    }

    /// 剪出 [start, end) 一段，重新編成 16 kHz mono MP3。
    pub async fn extract(&self, input: &Path, start: f64, end: f64, output: &Path) -> Result<()> {
        let (input, output) = (input.to_path_buf(), output.to_path_buf());
        blocking(move || {
            let mut src = Source::open(&input)?;
            let mut pos = src.seek(start);
            let mut enc = Mp3Writer::create(&output)?;
            let mut rs: Option<Resampler> = None;
            let mut out = Vec::new();
            let mut err = None;
            src.decode(&CancellationToken::new(), |pcm, rate| {
                let t0 = pos;
                pos += pcm.len() as f64 / rate as f64;
                if pos <= start {
                    return true;
                }
                let from = (((start - t0) * rate as f64).max(0.0) as usize).min(pcm.len());
                let to = (((end - t0) * rate as f64).max(0.0) as usize).min(pcm.len());
                if from < to {
                    let r = rs.get_or_insert_with(|| Resampler::new(rate));
                    out.clear();
                    r.push(&pcm[from..to], &mut out);
                    if let Err(e) = enc.write(&out) {
                        err = Some(e);
                        return false;
                    }
                }
                pos < end
            })?;
            if let Some(e) = err {
                return Err(e);
            }
            if let Some(mut r) = rs {
                out.clear();
                r.finish(&mut out);
                enc.write(&out)?;
            }
            enc.finish()
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Ffmpeg(format!("音訊處理中斷：{e}")))?
}

fn sym_err(path: &Path, e: SymError) -> Error {
    Error::Ffmpeg(format!("無法讀取「{}」：{e}", path.display()))
}

/// 一個音訊檔的第一條音軌。
struct Source {
    path: PathBuf,
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
}

impl Source {
    fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .map_err(|e| Error::Ffmpeg(format!("無法開啟「{}」：{e}", path.display())))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(|e| sym_err(path, e))?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| Error::Ffmpeg(format!("「{}」沒有可用的音軌", path.display())))?;
        let track_id = track.id;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| sym_err(path, e))?;
        Ok(Self {
            path: path.to_path_buf(),
            format,
            decoder,
            track_id,
        })
    }

    fn declared_duration(&self) -> Option<f64> {
        let t = self.format.tracks().iter().find(|t| t.id == self.track_id)?;
        let p = &t.codec_params;
        let frames = p.n_frames?;
        let rate = p.sample_rate?;
        (frames > 0 && rate > 0).then(|| frames as f64 / rate as f64)
    }

    /// 跳到 `secs` 附近，回傳實際落點（秒）；跳不了就從頭。
    fn seek(&mut self, secs: f64) -> f64 {
        if secs <= 0.0 {
            return 0.0;
        }
        let to = SeekTo::Time {
            time: Time::from(secs),
            track_id: Some(self.track_id),
        };
        match self.format.seek(SeekMode::Accurate, to) {
            Ok(s) => {
                self.decoder.reset();
                let tb = self
                    .format
                    .tracks()
                    .iter()
                    .find(|t| t.id == self.track_id)
                    .and_then(|t| t.codec_params.time_base);
                match tb {
                    Some(tb) => {
                        let t = tb.calc_time(s.actual_ts);
                        t.seconds as f64 + t.frac
                    }
                    None => 0.0,
                }
            }
            Err(_) => 0.0,
        }
    }

    /// 依序把每個封包解成 mono f32 交給 `f`（第二個參數是取樣率）；`f` 回 false 就停。
    fn decode(&mut self, cancel: &CancellationToken, mut f: impl FnMut(&[f32], u32) -> bool) -> Result<()> {
        let mut mono = Vec::new();
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(SymError::ResetRequired) => break,
                Err(e) => return Err(sym_err(&self.path, e)),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                // 壞掉的封包跳過（ffmpeg 也是）
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(sym_err(&self.path, e)),
            };
            let spec = *decoded.spec();
            let channels = spec.channels.count().max(1);
            let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            mono.clear();
            mono.extend(
                buf.samples()
                    .chunks_exact(channels)
                    .map(|fr| fr.iter().sum::<f32>() / channels as f32),
            );
            if !mono.is_empty() && !f(&mono, spec.rate) {
                break;
            }
        }
        Ok(())
    }
}

/// 任意取樣率 → 16 kHz。降頻用盒型平均（順便擋掉混疊），升頻用線性內插；語音辨識夠用。
struct Resampler {
    /// 來源取樣數／輸出取樣數。
    step: f64,
    /// 下一個輸出點在來源的位置（絕對索引）。
    next: f64,
    /// `buf[0]` 的絕對索引。
    base: u64,
    buf: Vec<f32>,
}

impl Resampler {
    fn new(src_rate: u32) -> Self {
        Self {
            step: src_rate.max(1) as f64 / TARGET_RATE as f64,
            next: 0.0,
            base: 0,
            buf: Vec::new(),
        }
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        self.drain(out, false);
    }

    fn finish(&mut self, out: &mut Vec<f32>) {
        self.drain(out, true);
    }

    fn drain(&mut self, out: &mut Vec<f32>, last: bool) {
        let end = self.base + self.buf.len() as u64;
        let half = (self.step / 2.0).max(0.5);
        loop {
            let hi = if self.step > 1.0 {
                (self.next + half).ceil() as u64
            } else {
                self.next.floor() as u64 + 2
            };
            if hi > end && !(last && (self.next as u64) < end) {
                break;
            }
            let hi = hi.min(end);
            let v = if self.step > 1.0 {
                let lo = ((self.next - half).floor().max(self.base as f64)) as u64;
                let s = &self.buf[(lo - self.base) as usize..(hi - self.base) as usize];
                if s.is_empty() { 0.0 } else { s.iter().sum::<f32>() / s.len() as f32 }
            } else {
                let i = self.next.floor() as u64;
                let frac = (self.next - i as f64) as f32;
                let a = self.buf[(i - self.base) as usize];
                let b = if i + 1 < end { self.buf[(i + 1 - self.base) as usize] } else { a };
                a + (b - a) * frac
            };
            out.push(v);
            self.next += self.step;
        }
        // 丟掉之後用不到的來源取樣
        let keep_from = ((self.next - half).floor().max(0.0) as u64).saturating_sub(1).max(self.base);
        let drop = ((keep_from - self.base) as usize).min(self.buf.len());
        self.buf.drain(..drop);
        self.base += drop as u64;
    }
}

/// 照 ffmpeg `silencedetect`：振幅持續低於門檻至少 `min_secs` 才算。
struct SilenceDetector {
    rate: f64,
    threshold: f32,
    min_frames: u64,
    pos: u64,
    run_start: Option<u64>,
    out: Vec<(f64, f64)>,
}

impl SilenceDetector {
    fn new(rate: u32, noise_db: i32, min_secs: f64) -> Self {
        Self {
            rate: rate.max(1) as f64,
            threshold: 10f32.powf(noise_db as f32 / 20.0),
            min_frames: (min_secs * rate as f64).round().max(1.0) as u64,
            pos: 0,
            run_start: None,
            out: Vec::new(),
        }
    }

    fn push(&mut self, pcm: &[f32]) {
        for &s in pcm {
            if s.abs() < self.threshold {
                self.run_start.get_or_insert(self.pos);
            } else if let Some(st) = self.run_start.take() {
                self.close(st, self.pos);
            }
            self.pos += 1;
        }
    }

    fn close(&mut self, start: u64, end: u64) {
        if end - start >= self.min_frames {
            self.out.push((start as f64 / self.rate, end as f64 / self.rate));
        }
    }

    fn finish(mut self) -> Vec<(f64, f64)> {
        if let Some(st) = self.run_start.take() {
            self.close(st, self.pos);
        }
        self.out
    }
}

/// 16 kHz mono 32 kbps CBR MP3 寫檔。
struct Mp3Writer {
    enc: mp3lame_encoder::Encoder,
    file: std::io::BufWriter<std::fs::File>,
    pcm: Vec<i16>,
    buf: Vec<u8>,
    path: PathBuf,
}

impl Mp3Writer {
    fn create(path: &Path) -> Result<Self> {
        let lame = |e: &dyn std::fmt::Display| Error::Ffmpeg(format!("MP3 編碼器初始化失敗：{e}"));
        let mut b = Builder::new().ok_or_else(|| lame(&"LAME"))?;
        b.set_num_channels(1).map_err(|e| lame(&e))?;
        b.set_sample_rate(TARGET_RATE).map_err(|e| lame(&e))?;
        b.set_brate(Bitrate::Kbps64).map_err(|e| lame(&e))?;
        b.set_quality(Quality::Good).map_err(|e| lame(&e))?;
        let enc = b.build().map_err(|e| lame(&e))?;
        let file = std::fs::File::create(path)
            .map_err(|e| Error::Ffmpeg(format!("無法寫入「{}」：{e}", path.display())))?;
        Ok(Self {
            enc,
            file: std::io::BufWriter::new(file),
            pcm: Vec::new(),
            buf: Vec::new(),
            path: path.to_path_buf(),
        })
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        self.pcm.clear();
        self.pcm
            .extend(samples.iter().map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16));
        self.buf.clear();
        self.buf.reserve(mp3lame_encoder::max_required_buffer_size(self.pcm.len()));
        self.enc
            .encode_to_vec(MonoPcm(&self.pcm), &mut self.buf)
            .map_err(|e| Error::Ffmpeg(format!("MP3 編碼失敗：{e}")))?;
        self.file.write_all(&self.buf).map_err(|e| self.io(e))
    }

    fn finish(mut self) -> Result<()> {
        self.buf.clear();
        self.buf.reserve(7200);
        self.enc
            .flush_to_vec::<FlushNoGap>(&mut self.buf)
            .map_err(|e| Error::Ffmpeg(format!("MP3 編碼失敗：{e}")))?;
        self.file.write_all(&self.buf).map_err(|e| self.io(e))?;
        self.file.flush().map_err(|e| self.io(e))
    }

    fn io(&self, e: std::io::Error) -> Error {
        Error::Ffmpeg(format!("無法寫入「{}」：{e}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f64, freq: f32, amp: f32) -> Vec<f32> {
        (0..(rate as f64 * secs) as usize)
            .map(|i| (i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin() * amp)
            .collect()
    }

    fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|s| {
                let v = (s * 32767.0) as i16;
                std::iter::repeat_n(v, channels as usize).flat_map(|v| v.to_le_bytes())
            })
            .collect();
        let mut f = Vec::new();
        f.extend(b"RIFF");
        f.extend((36 + data.len() as u32).to_le_bytes());
        f.extend(b"WAVEfmt ");
        f.extend(16u32.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(channels.to_le_bytes());
        f.extend(rate.to_le_bytes());
        f.extend((rate * channels as u32 * 2).to_le_bytes());
        f.extend((channels * 2).to_le_bytes());
        f.extend(16u16.to_le_bytes());
        f.extend(b"data");
        f.extend((data.len() as u32).to_le_bytes());
        f.extend(data);
        std::fs::write(path, f).unwrap();
    }

    #[test]
    fn resampler_keeps_duration() {
        for rate in [8_000, 16_000, 44_100, 48_000] {
            let mut r = Resampler::new(rate);
            let mut out = Vec::new();
            for chunk in tone(rate, 3.0, 440.0, 0.5).chunks(1234) {
                r.push(chunk, &mut out);
            }
            r.finish(&mut out);
            let expect = 3 * TARGET_RATE as usize;
            assert!(out.len().abs_diff(expect) <= 2, "{rate}: {} vs {expect}", out.len());
            // 振幅沒有被平均掉（440 Hz 遠低於 8 kHz）
            let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.4, "{rate}: peak {peak}");
        }
    }

    #[test]
    fn silence_detector_matches_silencedetect_rules() {
        let rate = 16_000;
        let mut pcm = tone(rate, 2.0, 300.0, 0.5);
        pcm.extend(vec![0.0; rate as usize]); // 1 秒靜音
        pcm.extend(tone(rate, 1.0, 300.0, 0.5));
        pcm.extend(vec![0.0; rate as usize / 10]); // 0.1 秒，太短
        pcm.extend(tone(rate, 1.0, 300.0, 0.5));
        let mut d = SilenceDetector::new(rate, -35, 0.4);
        d.push(&pcm);
        let s = d.finish();
        // 正弦波過零點附近的取樣也很小，但連續不到門檻長度，不會被當成靜音
        assert_eq!(s.len(), 1, "{s:?}");
        assert!((s[0].0 - 2.0).abs() < 0.01 && (s[0].1 - 3.0).abs() < 0.01, "{s:?}");
    }

    #[tokio::test]
    async fn normalize_detect_extract_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.wav");
        let mut pcm = tone(44_100, 4.0, 220.0, 0.4);
        pcm.extend(vec![0.0; 44_100]);
        pcm.extend(tone(44_100, 3.0, 220.0, 0.4));
        write_wav(&src, 44_100, 2, &pcm);

        let a = NativeAudio;
        let d = a.probe_duration(&src).await.unwrap();
        assert!((d - 8.0).abs() < 0.01, "{d}");

        let mp3 = dir.path().join("audio.mp3");
        let seen = std::sync::Mutex::new(0f32);
        a.normalize(&src, &mp3, d, &|f| *seen.lock().unwrap() = f, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(*seen.lock().unwrap(), 1.0);
        let d2 = a.probe_duration(&mp3).await.unwrap();
        assert!((d2 - 8.0).abs() < 0.2, "{d2}");

        let s = a.detect_silences(&mp3, -35, 0.4, &CancellationToken::new()).await.unwrap();
        assert!(s.iter().any(|(st, en)| (st - 4.0).abs() < 0.15 && (en - 5.0).abs() < 0.15), "{s:?}");

        let clip = dir.path().join("clip.mp3");
        a.extract(&mp3, 5.5, 7.5, &clip).await.unwrap();
        let d3 = a.probe_duration(&clip).await.unwrap();
        assert!((d3 - 2.0).abs() < 0.2, "{d3}");
    }
}
