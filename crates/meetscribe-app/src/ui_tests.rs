//! 離屏渲染各畫面（不開視窗、不碰螢幕），存成 PNG 供目視檢查：`target/ui-shots/*.png`。

use crate::app::{MeetApp, View};
use crate::views::project::Tab;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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

    // 不連正式的中轉站（傳到其他裝置的畫面另外塞假資料）
    let settings = meetscribe_core::config::Settings {
        relay_url: meetscribe_core::config::RELAY_OFF.into(),
        ..Default::default()
    };
    settings.save(&paths).unwrap();
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
    // 改了還沒存：底部固定列顯示「有未儲存的變更」
    harness.state_mut().settings_form.draft.chunk_minutes += 5;
    shot(&mut harness, "02b_settings_dirty");
    {
        let st = harness.state_mut();
        st.settings_form.draft = st.settings.clone();
    }

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

    // 新版已下載好：側欄提示卡＋設定頁「關於與更新」
    harness.set_size(egui::vec2(1280.0, 3000.0));
    {
        let st = harness.state_mut();
        st.device_id = "K7Q2-9XMA".into();
        st.update = crate::worker::UpdateStatus::Ready(meetscribe_core::update::Ready {
            version: "1.2.0".into(),
            notes: "修正跟隨播放的時間；播放改用高品質音訊".into(),
            path: "setup.exe".into(),
            targeted: false,
        });
        st.view = View::Settings;
    }
    shot(&mut harness, "13_update_ready");

    // 傳到其他裝置：設定頁的配對清單、別台的配對請求、配對、傳送與進度
    use crate::worker::{SendWhat, TransferJob, TransferMsg};
    use meetscribe_core::transfer::{Inbox, PairRequest, Peer};
    let peer = |id: &str, name: &str| Peer { id: id.into(), name: name.into(), pubkey: String::new(), paired_at: chrono::Local::now() };
    harness.set_size(egui::vec2(1280.0, 3400.0));
    {
        let st = harness.state_mut();
        crate::views::transfer::handle(
            st,
            TransferMsg::State {
                enabled: true,
                device_id: "K7Q2-9XMA".into(),
                name: "辦公室電腦".into(),
                peers: vec![peer("HPPT-5ZKF", "Pixel 9"), peer("M3RA-77QC", "家裡的筆電")],
            },
        );
        st.view = View::Settings;
    }
    shot(&mut harness, "14_transfer_settings");
    harness.set_size(egui::vec2(1280.0, 820.0));
    {
        let st = harness.state_mut();
        let inbox = Inbox {
            pair_requests: vec![PairRequest {
                id: "r1".into(),
                from: "Q8WE-4TZN".into(),
                from_name: "Galaxy S26".into(),
                from_pubkey: String::new(),
                code: "089063".into(),
            }],
            ..Default::default()
        };
        crate::views::transfer::handle(st, TransferMsg::Inbox(Box::new(inbox)));
        st.view = View::Project;
        st.selected = Some(id);
        st.project_ui.tab = Tab::Summary;
    }
    shot(&mut harness, "15_pair_request");
    {
        let st = harness.state_mut();
        crate::views::transfer::handle(st, TransferMsg::Inbox(Box::default()));
        st.transfer.open_pairing();
    }
    shot(&mut harness, "16_pairing");
    {
        let st = harness.state_mut();
        st.transfer = Default::default();
        crate::views::transfer::handle(
            st,
            TransferMsg::State {
                enabled: true,
                device_id: "K7Q2-9XMA".into(),
                name: "辦公室電腦".into(),
                peers: vec![peer("HPPT-5ZKF", "Pixel 9"), peer("M3RA-77QC", "家裡的筆電")],
            },
        );
        let job = TransferJob { send: true, title: "2026-10-29 預算會議".into(), peer_name: "Pixel 9".into(), done: 23_000_000, total: 61_000_000 };
        crate::views::transfer::handle(st, TransferMsg::Progress { key: "k1".into(), job });
        st.transfer.open_send(SendWhat::Meeting(id), "2026-10-30 產品週會".into());
    }
    shot(&mut harness, "17_send_dialog");

    // 摘要頁：同步到其他裝置、收過別台的更新時可以還原上一版
    std::fs::write(paths.projects_dir().join(id.to_string()).join("previous-content.json"), "{}").unwrap();
    {
        let st = harness.state_mut();
        st.transfer = Default::default();
        crate::views::transfer::handle(
            st,
            TransferMsg::State {
                enabled: true,
                device_id: "K7Q2-9XMA".into(),
                name: "辦公室電腦".into(),
                peers: vec![peer("HPPT-5ZKF", "Pixel 9")],
            },
        );
        st.project_ui.tab = Tab::Summary;
    }
    shot(&mut harness, "18_summary_sync");

    // 摘要模型下拉選單：免費組在前、各組推薦的在前，標免費／付費與費率
    let m = |id: &str, name: &str, p: f64, c: f64| meetscribe_core::openrouter::ModelInfo {
        id: id.into(),
        name: name.into(),
        context_length: 0,
        prompt_price: p,
        completion_price: c,
        input_modalities: vec!["text".into()],
        output_modalities: vec!["text".into()],
    };
    {
        let st = harness.state_mut();
        st.or_models = Some(Ok(vec![
            m("anthropic/claude-sonnet-5.5", "Anthropic: Claude Sonnet 5.5", 0.000003, 0.00001),
            m("deepseek/deepseek-v4.1-flash", "DeepSeek: V4.1 Flash", 0.0000001, 0.0000005),
            m("nvidia/nemotron-3-ultra-550b-a55b:free", "NVIDIA: Nemotron 3 Ultra (free)", 0.0, 0.0),
            m("google/gemma-4-31b-it:free", "Google: Gemma 4 31B (free)", 0.0, 0.0),
            m("some/other:free", "Other Model (free)", 0.0, 0.0),
            m("zzz/cheap", "Cheap Model", 0.00000005, 0.0000001),
        ]));
        st.view = View::Settings;
        st.settings_form = crate::views::settings::SettingsForm::new(&st.settings);
    }
    harness.set_size(egui::vec2(1280.0, 1300.0));
    harness.run_steps(4);
    harness.get_by_label("摘要模型").click();
    shot(&mut harness, "19_summary_models");
}
