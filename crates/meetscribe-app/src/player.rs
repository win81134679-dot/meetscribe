//! 播放正規化後的會議音訊（核對逐字稿、聽說話者樣本）。沒有音效裝置時整個功能停用，不影響其他操作。

use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct AudioPlayer {
    _sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    loaded: Option<PathBuf>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self, String> {
        let sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| format!("找不到音效裝置：{e}"))?;
        let player = rodio::Player::connect_new(sink.mixer());
        Ok(Self {
            _sink: sink,
            player,
            loaded: None,
        })
    }

    /// 從 `secs` 秒開始播放 `path`。
    pub fn play_from(&mut self, path: &Path, secs: f64) -> Result<(), String> {
        if self.loaded.as_deref() != Some(path) || self.player.empty() {
            self.player.clear();
            let file = std::fs::File::open(path).map_err(|e| format!("開不了音訊檔：{e}"))?;
            let source = rodio::Decoder::try_from(file).map_err(|e| format!("無法解碼：{e}"))?;
            self.player.append(source);
            self.loaded = Some(path.to_path_buf());
        }
        let _ = self.player.try_seek(Duration::from_secs_f64(secs.max(0.0)));
        self.player.play();
        Ok(())
    }

    pub fn toggle(&mut self) {
        if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub fn stop(&mut self) {
        self.player.clear();
        self.loaded = None;
    }

    pub fn is_playing(&self) -> bool {
        !self.player.is_paused() && !self.player.empty()
    }

    pub fn has_track(&self) -> bool {
        !self.player.empty()
    }

    pub fn loaded(&self) -> Option<&Path> {
        self.loaded.as_deref()
    }

    pub fn position(&self) -> f64 {
        self.player.get_pos().as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use rodio::Source;
    use std::time::Duration;

    /// 跟隨播放靠「播放到幾秒」：用我們的音訊格式（16 kHz 單聲道 32 kbps MP3）在第 40.0 秒放一聲嗶，
    /// 從不同位置開始播，量嗶聲實際出現的時間。需要 ffmpeg（同 e2e 測試）。
    #[test]
    fn seek_and_position_match_the_audio() {
        let dir = tempfile::tempdir().unwrap();
        // 轉錄用的 16 kHz MP3 與播放用的 AAC（m4a）兩種都要準
        for (name, codec) in [("beep.mp3", ["-ar", "16000", "-b:a", "64k"]), ("beep.m4a", ["-c:a", "aac", "-b:a", "128k"])] {
        let mp3 = dir.path().join(name);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "anullsrc=r=16000:cl=mono:d=60", "-f", "lavfi", "-i",
                   "sine=f=1000:r=16000:d=1", "-filter_complex", "[1]adelay=40000|40000[b];[0][b]amix=inputs=2:duration=first:normalize=0",
                   "-ac", "1"])
            .args(codec)
            .arg(&mp3)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("沒有 ffmpeg，略過");
            return;
        }
        // 播放器的位置＝跳轉目標＋之後播了幾個樣本（rodio 的 get_pos），所以要驗的是：解碼器跳到 t 之後，
        // 拿到的第一個樣本真的是 t 秒的聲音。
        for seek in [39.0, 12.5, 39.9, 0.0] {
            let mut d = rodio::Decoder::try_from(std::fs::File::open(&mp3).unwrap()).unwrap();
            d.try_seek(Duration::from_secs_f64(seek)).unwrap();
            let rate = d.sample_rate().get() as f64;
            let channels = d.channels().get() as usize;
            let n = d.by_ref().position(|s| s.abs() > 0.05).expect("聽得到嗶聲");
            let onset = seek + (n / channels) as f64 / rate;
            eprintln!("{name} seek {seek}: 嗶聲在 {onset:.3}s");
            assert!((onset - 40.0).abs() < 0.1, "{name} 跳到 {seek}s 後嗶聲出現在 {onset:.3}s（應為 40.0）");
        }
        }
    }
}
