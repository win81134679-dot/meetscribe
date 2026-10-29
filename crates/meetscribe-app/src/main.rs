// 發佈版不要多開一個主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod player;
mod theme;
#[cfg(test)]
mod ui_tests;
mod views;
mod widgets;
mod worker;

use meetscribe_core::config::AppPaths;

fn main() -> eframe::Result<()> {
    let paths = AppPaths::detect();
    if let Err(e) = paths.ensure() {
        eprintln!("無法建立程式資料夾 {}：{e}", paths.root.display());
    }
    init_logging(&paths);
    tracing::info!(
        "MeetScribe {} 啟動，資料夾 {}",
        env!("CARGO_PKG_VERSION"),
        paths.root.display()
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MeetScribe 會議逐字稿")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "MeetScribe",
        options,
        Box::new(move |cc| Ok(Box::new(app::MeetApp::new(cc, paths)))),
    )
}

/// 紀錄寫到 `logs\meetscribe.log`（附加），超過 5 MB 先換成 .old。
fn init_logging(paths: &AppPaths) {
    let file = paths.logs_dir().join("meetscribe.log");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        let _ = std::fs::rename(&file, file.with_extension("log.old"));
    }
    let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(f))
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
}
