//! MeetScribe 命令列：批次轉錄、重新摘要、匯出。與桌面程式共用設定、金鑰與專案資料夾。

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use meetscribe_core::config::{AppPaths, LlmProvider, Settings};
use meetscribe_core::export;
use meetscribe_core::model::{JobOptions, Project, ProjectStatus};
use meetscribe_core::pipeline::{Engine, Event, EventSink, LogLevel};
use meetscribe_core::secrets::ApiKeys;
use meetscribe_core::store::ProjectStore;
use meetscribe_core::textutil::fmt_duration_zh;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "meetscribe", version, about = "會議錄音 → 完整逐字稿（含說話者）＋ 會議摘要")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 轉錄一個或多個錄音檔，完成後匯出
    Transcribe {
        files: Vec<PathBuf>,
        /// 語言代碼（zh、en、ja…），不填自動判斷
        #[arg(long)]
        language: Option<String>,
        /// 不辨識說話者
        #[arg(long)]
        no_speakers: bool,
        /// 不產生摘要
        #[arg(long)]
        no_summary: bool,
        /// 與會者名單檔（一行一位）
        #[arg(long)]
        participants: Option<PathBuf>,
        /// 專有名詞，逗號分隔
        #[arg(long)]
        keyterms: Option<String>,
        /// 摘要的額外要求
        #[arg(long)]
        instructions: Option<String>,
        /// 摘要方式：free（預設，Gemini／OpenRouter 免費模型）、claude（本機 Claude Code CLI）、openrouter
        #[arg(long)]
        summary: Option<String>,
        /// 匯出格式，逗號分隔（md,docx,txt,srt,vtt,csv,json）；不填用設定值
        #[arg(long)]
        formats: Option<String>,
        /// 匯出資料夾；不填放在錄音檔旁邊
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// 列出所有專案
    List,
    /// 從中斷處繼續處理
    Resume { id: String },
    /// 重新產生摘要
    Summarize {
        id: String,
        #[arg(long)]
        instructions: Option<String>,
        #[arg(long)]
        summary: Option<String>,
    },
    /// 匯出專案
    Export {
        id: String,
        #[arg(long)]
        formats: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn summary_provider(name: &str) -> Result<LlmProvider> {
    Ok(match name {
        "free" | "google" | "gemini" => LlmProvider::Google,
        "claude" | "claude-code" => LlmProvider::ClaudeCode,
        "openrouter" => LlmProvider::OpenRouter,
        other => bail!("不認得的摘要方式：{other}（free／claude／openrouter）"),
    })
}

fn with_summary(mut s: Settings, choice: Option<&str>) -> Result<Settings> {
    if let Some(c) = choice {
        let p = summary_provider(c)?;
        if p != s.summary_provider {
            s.summary_provider = p;
            s.summary_model = p.default_summary_model().into();
        }
    }
    Ok(s)
}

fn sink() -> EventSink {
    Arc::new(|e| match e {
        Event::Progress {
            stage,
            fraction,
            message,
            ..
        } => {
            eprintln!("  [{:>3.0}%] {}：{}", fraction * 100.0, stage.label(), message);
        }
        Event::Log { level, message, .. } => {
            let tag = match level {
                LogLevel::Info => "·",
                LogLevel::Warn => "!",
                LogLevel::Error => "✗",
            };
            eprintln!("  {tag} {message}");
        }
        Event::Updated(_) => {}
    })
}

fn find(store: &ProjectStore, id: &str) -> Result<Project> {
    let all = store.list();
    let hits: Vec<&Project> = all
        .iter()
        .filter(|p| p.id.to_string().starts_with(id) || p.title == id)
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("找不到專案 {id}（用 list 查 ID）"),
        _ => bail!("{id} 對到多個專案，請給更長的 ID"),
    }
}

fn do_export(p: &Project, settings: &Settings, formats: Option<&str>, out: Option<PathBuf>) -> Result<()> {
    let ids: Vec<String> = match formats {
        Some(f) => f
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => settings.export_formats.clone(),
    };
    let dir = out.unwrap_or_else(|| meetscribe_core::config::default_export_dir(settings, &p.source_file));
    for f in export::export_all(p, &ids, &dir)? {
        println!("{}", f.display());
    }
    Ok(())
}

fn cancel_on_ctrl_c() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("取消中…（已完成的部分會保留，可用 resume 繼續）");
            t.cancel();
        }
    });
    token
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .init();
    let cli = Cli::parse();
    let paths = AppPaths::detect();
    paths.ensure()?;
    let settings = Settings::load(&paths);
    let store = ProjectStore::new(paths.projects_dir());

    match cli.cmd {
        Command::List => {
            for p in store.list() {
                println!(
                    "{}  {:<6} {:>10}  {}",
                    &p.id.to_string()[..8],
                    p.status.label(),
                    fmt_duration_zh(p.duration),
                    p.title
                );
            }
        }
        Command::Transcribe {
            files,
            language,
            no_speakers,
            no_summary,
            participants,
            keyterms,
            instructions,
            summary,
            formats,
            out,
        } => {
            if files.is_empty() {
                bail!("請給至少一個錄音檔");
            }
            let settings = with_summary(settings, summary.as_deref())?;
            let participants = match participants {
                Some(f) => std::fs::read_to_string(&f)
                    .with_context(|| format!("讀不了 {}", f.display()))?
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect(),
                None => Vec::new(),
            };
            let options = JobOptions {
                language,
                identify_speakers: !no_speakers,
                expected_speakers: None,
                participants,
                keyterms: keyterms
                    .map(|k| {
                        k.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default(),
                generate_summary: !no_summary,
                summary_instructions: instructions.unwrap_or_default(),
            };
            let engine = Engine::from_settings(&paths, settings.clone(), ApiKeys::load()).await?;
            let cancel = cancel_on_ctrl_c();
            let mut failed = 0;
            for f in files {
                let f = std::fs::canonicalize(&f).with_context(|| format!("找不到 {}", f.display()))?;
                eprintln!("▶ {}", f.display());
                let p = engine
                    .run(Project::new(f, options.clone()), sink(), cancel.clone())
                    .await;
                match &p.status {
                    ProjectStatus::Done => {
                        eprintln!(
                            "✔ 完成：{} 位說話者、{} 句，費用 US${:.4}（ID {}）",
                            p.speakers.len(),
                            p.segments.len(),
                            p.usage.total_usd(),
                            &p.id.to_string()[..8]
                        );
                        do_export(&p, &settings, formats.as_deref(), out.clone())?;
                    }
                    other => {
                        failed += 1;
                        eprintln!("✗ {}（ID {}，可用 resume 繼續）", other.label(), &p.id.to_string()[..8]);
                    }
                }
                if cancel.is_cancelled() {
                    break;
                }
            }
            if failed > 0 {
                std::process::exit(1);
            }
        }
        Command::Resume { id } => {
            let p = find(&store, &id)?;
            let engine = Engine::from_settings(&paths, settings, ApiKeys::load()).await?;
            let p = engine.run(p, sink(), cancel_on_ctrl_c()).await;
            println!("{}", p.status.label());
            if p.status != ProjectStatus::Done {
                std::process::exit(1);
            }
        }
        Command::Summarize {
            id,
            instructions,
            summary,
        } => {
            let p = find(&store, &id)?;
            let settings = with_summary(settings, summary.as_deref())?;
            let engine = Engine::from_settings(&paths, settings, ApiKeys::load()).await?;
            for n in &engine.notices {
                eprintln!("  · {n}");
            }
            let p = engine
                .regenerate_summary(p, instructions.unwrap_or_default(), sink(), cancel_on_ctrl_c())
                .await?;
            println!("{}", p.summary.map(|s| s.markdown).unwrap_or_default());
        }
        Command::Export { id, formats, out } => {
            let p = find(&store, &id)?;
            do_export(&p, &settings, formats.as_deref(), out)?;
        }
    }
    Ok(())
}
