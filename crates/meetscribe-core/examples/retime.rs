//! 用新的時間規則重新對齊一個專案資料夾（就地改寫 project.json）。先複製一份再跑：
//! `cargo run -p meetscribe-core --example retime -- <專案資料夾的複本>`
use meetscribe_core::audio::AudioTool;
use meetscribe_core::model::Project;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("專案資料夾"));
    let file = dir.join("project.json");
    let mut p: Project = serde_json::from_str(&std::fs::read_to_string(&file).expect("讀 project.json")).expect("格式");
    p.timing_rev = 0;
    let audio = AudioTool::locate("").expect("ffmpeg");
    let t = std::time::Instant::now();
    let changed = meetscribe_core::pipeline::retime_project(&audio, &dir, &mut p, &CancellationToken::new())
        .await
        .expect("retime");
    std::fs::write(&file, serde_json::to_string_pretty(&p).unwrap()).unwrap();
    println!("changed={changed} segments={} in {:.1}s", p.segments.len(), t.elapsed().as_secs_f64());
}
