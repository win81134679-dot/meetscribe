//! 真的呼叫本機 Claude Code CLI 產生摘要。會用到使用者的 Claude 額度，預設不跑：
//! `cargo test -p meetscribe-core --test claude_cli_live -- --ignored`

use meetscribe_core::claude_code::ClaudeCodeCli;
use meetscribe_core::config::{LlmProvider, Settings};
use meetscribe_core::model::{JobOptions, Project, Segment, Timing};
use meetscribe_core::prompts::Prompts;
use meetscribe_core::summary;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "呼叫真的 Claude Code CLI"]
async fn summary_via_local_claude_code() {
    let mut p = Project::new("週會.m4a".into(), JobOptions::default());
    p.duration = 95.0;
    p.add_speaker(Some("王經理".into()), String::new());
    p.add_speaker(Some("小李".into()), String::new());
    let lines = [
        ("S1", "今天確認第四季預算，上限五十萬。"),
        ("S2", "報價我下週五前交。"),
        ("S1", "好，那壓力測試十一月十五日前要做完，小李負責。"),
    ];
    for (i, (sp, t)) in lines.iter().enumerate() {
        p.segments.push(Segment {
            id: i as u64 + 1,
            start: i as f64 * 30.0,
            end: i as f64 * 30.0 + 25.0,
            timing: Timing::Model,
            speaker: Some(sp.to_string()),
            text: t.to_string(),
            chunk: 0,
            edited: false,
        });
    }
    let tmp = tempfile::tempdir().unwrap();
    let settings = Settings {
        summary_provider: LlmProvider::ClaudeCode,
        summary_model: "sonnet".into(),
        ..Default::default()
    };
    let cli = ClaudeCodeCli::new(&settings, tmp.path().join("claude-work")).expect("找得到 claude");
    let out = summary::summarize(
        &p,
        &cli,
        &settings,
        &Prompts::default(),
        "",
        &|_, _| {},
        &CancellationToken::new(),
    )
    .await
    .expect("摘要成功");
    println!("{}", out.summary.markdown);
    assert!(out.summary.markdown.contains("50") || out.summary.markdown.contains("五十"));
    assert!(out.summary.markdown.contains("小李"));
}
