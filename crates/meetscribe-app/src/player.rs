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
