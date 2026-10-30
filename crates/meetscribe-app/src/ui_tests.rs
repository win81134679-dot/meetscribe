//! 離屏渲染各畫面（不開視窗、不碰螢幕），存成 PNG 供目視檢查：`target/ui-shots/*.png`。

use crate::app::{MeetApp, View};
use crate::views::project::Tab;
use egui_kittest::Harness;
use meetscribe_core::config::AppPaths;
use meetscribe_core::model::{JobOptions, Project, ProjectStatus, Segment, Stage, Summary, Timing};
use meetscribe_core::store::ProjectStore;
use std::path::PathBuf;

fn sample_project() -> Project {
    let mut p = Project::new(PathBuf::from(r"C:\會議\2026-10-30 產品週會.m4a"), JobOptions::default());
    p.title = "2026-10-30 產品週會".into();
    p.duration = 3725.0;
    p.status = ProjectStatus::Done;
    p.stage = Stage::Done;
    p.audio_file = Some("audio.mp3".into());
    p.add_speaker(Some("王經理（主持人）".into()), "男聲，主持會議".into());
    p.add_speaker(Some("陳工程師".into()), "女聲".into());
    p.add_speaker(None, String::new());
    let lines = [
        ("S1", "大家好，今天主要討論第四季的上線時程和預算。"),
        ("S2", "後端的部分目前進度大概八成，API 文件下週一可以給。"),
        ("S1", "那壓力測試什麼時候做？"),
        ("S2", "預計十一月第二週，需要再兩台測試機。"),
        ("S3", "預算這邊沒問題，五十萬內都可以核。"),
        ("S1", "好，那就這樣決定：十一月十五日前完成壓測，陳工程師負責。"),
    ];
    for (i, (sp, text)) in lines.iter().enumerate() {
        p.segments.push(Segment {
            id: i as u64 + 1,
            start: i as f64 * 37.0,
            end: i as f64 * 37.0 + 30.0,
            timing: if i == 4 { Timing::Estimated } else { Timing::Model },
            speaker: Some(sp.to_string()),
            text: text.to_string(),
            chunk: 0,
            edited: i == 3,
        });
    }
    p.summary = Some(Summary {
        markdown: "這場週會確認了第四季上線時程與預算，決定 11/15 前完成壓力測試。\n\n## 決議\n- 壓力測試 **11 月 15 日前**完成\n- 預算上限 50 萬\n\n## 待辦\n| 事項 | 負責人 | 期限 |\n|---|---|---|\n| API 文件 | 陳工程師 | 下週一 |\n| 壓力測試 | 陳工程師 | 11/15 |\n".into(),
        model: "gemini-3.8-flash".into(),
        created_at: chrono::Local::now(),
        instructions: String::new(),
    });
    p.usage.stt_usd = 0.124;
    p.models.stt = "nvidia/nemotron-3.5-transcribe".into();
    p.models.attribution = "gemini-3.8-flash".into();
    p
}

fn shot(harness: &mut Harness<'_, MeetApp>, name: &str) {
    harness.run_steps(6);
    let img = harness.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-shots");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join(format!("{name}.png"))).unwrap();
}

#[test]
fn render_screens() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::at(tmp.path().to_path_buf());
    paths.ensure().unwrap();
    let project = sample_project();
    ProjectStore::new(paths.projects_dir()).save(&project).unwrap();
    let mut running = Project::new(PathBuf::from(r"C:\會議\全員大會.mp4"), JobOptions::default());
    running.title = "全員大會（排隊中）".into();
    running.created_at -= chrono::Duration::hours(1);
    ProjectStore::new(paths.projects_dir()).save(&running).unwrap();

    let p2 = paths.clone();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 820.0))
        .wgpu()
        .build_eframe(move |cc| MeetApp::new(cc, p2.clone()));

    harness.ctx.set_theme(egui::Theme::Light);
    harness.state_mut().view = View::NewJob;
    harness.state_mut().has_openrouter_key = false;
    shot(&mut harness, "01_new_job");
    {
        let st = harness.state_mut();
        st.has_openrouter_key = true;
        st.settings_form.or_status = Some(Ok("金鑰有效，帳戶餘額 US$4.87，累計用量 US$0.13".into()));
    }
    shot(&mut harness, "01b_new_job_with_key");

    harness.state_mut().view = View::Settings;
    shot(&mut harness, "02_settings");

    let id = project.id;
    for (tab, name) in [
        (Tab::Summary, "03_summary"),
        (Tab::Transcript, "04_transcript"),
        (Tab::Speakers, "05_speakers"),
        (Tab::Export, "06_export"),
        (Tab::Log, "07_log"),
    ] {
        let st = harness.state_mut();
        st.view = View::Project;
        st.selected = Some(id);
        st.project_ui.tab = tab;
        shot(&mut harness, name);
    }

    // 深色模式（跟隨 Windows）
    harness.ctx.set_theme(egui::Theme::Dark);
    harness.state_mut().project_ui.tab = Tab::Transcript;
    shot(&mut harness, "08_dark_transcript");
    harness.state_mut().view = View::NewJob;
    shot(&mut harness, "09_dark_new_job");

    // 長頁面整頁（輸出選項在折疊線以下）
    harness.ctx.set_theme(egui::Theme::Light);
    harness.set_size(egui::vec2(1280.0, 2000.0));
    harness.state_mut().view = View::Settings;
    shot(&mut harness, "10_settings_full");
    harness.state_mut().view = View::NewJob;
    shot(&mut harness, "11_new_job_full");
    {
        let st = harness.state_mut();
        st.view = View::Project;
        st.selected = Some(id);
        st.project_ui.tab = Tab::Export;
    }
    shot(&mut harness, "12_export_full");
}
