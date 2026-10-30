//! 處理流程：前處理 → 轉錄 → 說話者分配 → 整合 → 摘要。
//!
//! 每一步完成就存檔；中斷（失敗、取消、關程式）後再跑同一個專案，會從還沒做完的地方接著做。

use crate::attribution::{self, KnownSpeaker, ROSTER_LIMIT};
use crate::audio::{self, Ffmpeg, NORMALIZED_EXT, NORMALIZED_FORMAT};
use crate::claude_code::ClaudeCodeCli;
use crate::config::{AppPaths, LlmProvider, Settings};
use crate::export;
use crate::gemini::{self, Gemini};
use crate::model::{ChunkRecord, Project, ProjectStatus, Segment, Stage, SttResult, Timing};
use crate::openrouter::{OpenRouter, SttCapabilities};
use crate::prompts::Prompts;
use crate::providers::{LlmClient, TranscribeRequest, Transcriber};
use crate::secrets::ApiKeys;
use crate::store::ProjectStore;
use crate::summary;
use crate::textutil::{fmt_clock, weight};
use crate::units::{estimate_times, parse_spk_tags, split_units, times_from_words};
use crate::{Error, Result};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// 一句最多幾個字（說話者分配的最小單位）。
const UNIT_MAX_CHARS: usize = 80;
/// 靜音切點的搜尋範圍（秒）。
const CUT_WINDOW_SECS: f64 = 90.0;
/// 比這更短的段落遇到供應商逾時就不再切，直接回報錯誤。
const MIN_SPLIT_SECS: f64 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub enum Event {
    Progress {
        id: Uuid,
        stage: Stage,
        fraction: f32,
        message: String,
    },
    Log {
        id: Uuid,
        level: LogLevel,
        message: String,
    },
    /// 專案有新的狀態（已存檔）。
    Updated(Box<Project>),
}

pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

/// 一段發言：(段內說話者標籤, 文字, 段內起訖秒數)。
type Turn = (String, String, Option<(f64, f64)>);

pub struct Engine {
    pub settings: Settings,
    pub prompts: Prompts,
    pub transcriber: Arc<dyn Transcriber>,
    /// 說話者分配（要能聽音訊）。
    pub attribution_llm: Arc<dyn LlmClient>,
    /// 說話者整合與摘要（純文字）。
    pub text_llm: Arc<dyn LlmClient>,
    pub ffmpeg: Ffmpeg,
    pub store: ProjectStore,
    pub stt_caps: Option<SttCapabilities>,
    /// 建立引擎時的提醒（例如沒有 Google Key 改用 OpenRouter 免費模型），每次執行會寫進紀錄。
    pub notices: Vec<String>,
}

/// 傳進各步驟的共用參數。
struct Ctx<'a> {
    sink: &'a EventSink,
    cancel: &'a CancellationToken,
}

impl Ctx<'_> {
    fn progress(&self, p: &Project, stage: Stage, fraction: f32, message: impl Into<String>) {
        (self.sink)(Event::Progress {
            id: p.id,
            stage,
            fraction,
            message: message.into(),
        });
    }
    fn log(&self, p: &Project, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        match level {
            LogLevel::Info => tracing::info!("[{}] {message}", p.title),
            LogLevel::Warn => tracing::warn!("[{}] {message}", p.title),
            LogLevel::Error => tracing::error!("[{}] {message}", p.title),
        }
        (self.sink)(Event::Log {
            id: p.id,
            level,
            message,
        });
    }
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Engine {
    /// 依設定建立引擎：語音轉文字走 OpenRouter；聊天模型依角色走 Google AI Studio 或 OpenRouter。
    /// 選了 Google 卻沒有 Google Key 時，改用 OpenRouter 上的免費模型並記下提醒。
    pub async fn from_settings(paths: &AppPaths, mut settings: Settings, keys: ApiKeys) -> Result<Self> {
        let ffmpeg = Ffmpeg::locate(&settings.ffmpeg_path)?;
        let client = OpenRouter::new(&settings, keys.openrouter.clone())?;
        let caps = match client.stt_capabilities(&settings.stt_model).await {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("查不到 {} 的供應商資訊：{e}", settings.stt_model);
                None
            }
        };
        let openrouter = Arc::new(match &caps {
            Some(c) => client.with_stt_capabilities(c),
            None => client,
        });
        let mut notices = Vec::new();
        let google: Option<Arc<Gemini>> = match keys.google.as_deref().filter(|k| !k.trim().is_empty()) {
            Some(k) => Some(Arc::new(Gemini::new(&settings, k.to_string())?)),
            None => None,
        };

        let available = match &google {
            Some(g) => g.list_models().await.ok(),
            None => None,
        };
        // 模型名稱會隨 Google 改版變動：設定的不存在就挑目前最好的 Flash
        let google_model = |current: &mut String, role: &str, notices: &mut Vec<String>| {
            if let Some(list) = &available
                && let Some(m) = gemini::pick_audio_model(current, list)
                && m != *current
            {
                notices.push(format!("Google 沒有 {current}，{role}改用 {m}"));
                *current = m;
            }
        };

        let attribution_llm: Arc<dyn LlmClient> = match (settings.attribution_provider, &google) {
            (LlmProvider::Google, Some(g)) => {
                google_model(&mut settings.attribution_model, "說話者分配", &mut notices);
                g.clone()
            }
            (LlmProvider::OpenRouter, _) => openrouter.clone(),
            (_, _) => {
                settings.attribution_model = LlmProvider::OpenRouter.default_attribution_model().into();
                notices.push(format!(
                    "尚未設定 Google AI Studio Key，說話者分配改用 OpenRouter 免費模型 {}",
                    settings.attribution_model
                ));
                openrouter.clone()
            }
        };
        let text_llm: Arc<dyn LlmClient> = match (settings.summary_provider, &google) {
            (LlmProvider::Google, Some(g)) => {
                google_model(&mut settings.summary_model, "摘要", &mut notices);
                g.clone()
            }
            (LlmProvider::Google, None) => {
                settings.summary_model = LlmProvider::OpenRouter.default_summary_model().into();
                notices.push(format!(
                    "尚未設定 Google AI Studio Key，摘要改用 OpenRouter 免費模型 {}",
                    settings.summary_model
                ));
                openrouter.clone()
            }
            (LlmProvider::OpenRouter, _) => openrouter.clone(),
            (LlmProvider::ClaudeCode, _) => match ClaudeCodeCli::new(&settings, paths.claude_workdir()) {
                Ok(c) => Arc::new(c),
                Err(e) => {
                    settings.summary_model = LlmProvider::OpenRouter.default_summary_model().into();
                    notices.push(format!("{e}；摘要改用 OpenRouter 免費模型 {}", settings.summary_model));
                    openrouter.clone()
                }
            },
        };
        Ok(Self {
            prompts: Prompts::load(&paths.prompts_dir()),
            transcriber: openrouter,
            attribution_llm,
            text_llm,
            ffmpeg,
            store: ProjectStore::new(paths.projects_dir()),
            stt_caps: caps,
            notices,
            settings,
        })
    }

    fn save(&self, p: &mut Project, sink: &EventSink) -> Result<()> {
        p.touch();
        self.store.save(p)?;
        sink(Event::Updated(Box::new(p.clone())));
        Ok(())
    }

    /// 跑完整流程（或從斷點續跑）。錯誤記在 `status`，不往外丟。
    pub async fn run(&self, mut project: Project, sink: EventSink, cancel: CancellationToken) -> Project {
        // 已經完成、這次又沒有任何變動（例如再按一次續跑）就不重複匯出
        let before = (project.status == ProjectStatus::Done)
            .then(|| (project.segments.clone(), project.speakers.clone(), project.summary.clone()));
        project.status = ProjectStatus::Running;
        project.models.stt = self.transcriber.model().to_string();
        project.models.attribution = self.settings.attribution_model.clone();
        project.models.summary = self.settings.summary_model.clone();
        let _ = self.save(&mut project, &sink);
        let ctx = Ctx {
            sink: &sink,
            cancel: &cancel,
        };
        for n in &self.notices {
            ctx.log(&project, LogLevel::Info, n);
        }
        let result = self.run_stages(&mut project, &ctx).await;
        project.status = match result {
            Ok(()) => {
                project.stage = Stage::Done;
                project.status = ProjectStatus::Done; // 匯出的 JSON 要記成完成
                let unchanged = before.is_some_and(|(segments, speakers, summary)| {
                    segments == project.segments && speakers == project.speakers && summary == project.summary
                });
                if !unchanged {
                    self.auto_export(&mut project, &ctx);
                }
                ProjectStatus::Done
            }
            Err(Error::Cancelled) => {
                ctx.log(&project, LogLevel::Warn, "已取消；之後可從中斷處繼續");
                ProjectStatus::Cancelled
            }
            Err(e) => {
                ctx.log(&project, LogLevel::Error, e.to_string());
                ProjectStatus::Failed(e.to_string())
            }
        };
        if let Err(e) = self.save(&mut project, &sink) {
            tracing::error!("存檔失敗：{e}");
        }
        project
    }

    /// 依專案（或目前設定）的輸出選項匯出。失敗只記警告，不影響「完成」。
    fn auto_export(&self, p: &mut Project, ctx: &Ctx<'_>) {
        let opts = p.options.export.clone().unwrap_or_else(|| self.settings.export.clone());
        if !opts.auto || opts.known_formats().is_empty() {
            return;
        }
        match export::export_project(p, &opts) {
            Ok(files) => {
                ctx.log(
                    p,
                    LogLevel::Info,
                    format!("已匯出 {} 個檔案到 {}", files.len(), opts.target_dir(p).display()),
                );
                p.exported = files;
            }
            Err(e) => {
                let msg = format!("自動匯出失敗：{e}（可到「匯出」頁手動匯出）");
                ctx.log(p, LogLevel::Warn, &msg);
                p.warnings.push(msg);
            }
        }
    }

    async fn run_stages(&self, p: &mut Project, ctx: &Ctx<'_>) -> Result<()> {
        p.stage = Stage::Prepare;
        self.prepare(p, ctx).await?;
        p.stage = Stage::Transcribe;
        self.transcribe(p, ctx).await?;
        p.stage = Stage::Speakers;
        self.build_segments(p, ctx).await?;
        if p.options.identify_speakers && !p.speakers_reconciled {
            p.stage = Stage::Reconcile;
            self.reconcile(p, ctx).await?;
        }
        if p.options.generate_summary && p.summary.is_none() {
            p.stage = Stage::Summary;
            let instructions = p.options.summary_instructions.clone();
            self.summarize_into(p, &instructions, ctx).await?;
        }
        Ok(())
    }

    // ---------- 前處理 ----------

    async fn prepare(&self, p: &mut Project, ctx: &Ctx<'_>) -> Result<()> {
        let dir = self.store.dir(p.id);
        std::fs::create_dir_all(dir.join("chunks"))?;
        std::fs::create_dir_all(dir.join("clips"))?;
        let audio_rel = format!("audio.{NORMALIZED_EXT}");
        let audio_path = dir.join(&audio_rel);
        if p.audio_file.is_none() || !audio_path.is_file() {
            if !p.source_file.is_file() {
                return Err(Error::Other(format!("找不到原始檔案：{}", p.source_file.display())));
            }
            ctx.progress(p, Stage::Prepare, 0.0, "讀取音訊資訊");
            let src_duration = self.ffmpeg.probe_duration(&p.source_file).await?;
            ctx.log(p, LogLevel::Info, format!("原始音訊長度 {}", fmt_clock(src_duration)));
            let pid = p.id;
            let sink = ctx.sink.clone();
            let report = move |f: f32| {
                sink(Event::Progress {
                    id: pid,
                    stage: Stage::Prepare,
                    fraction: f * 0.8,
                    message: "轉成 16 kHz 單聲道".into(),
                });
            };
            self.ffmpeg
                .normalize(&p.source_file, &audio_path, src_duration, &report, ctx.cancel)
                .await?;
            p.duration = self.ffmpeg.probe_duration(&audio_path).await.unwrap_or(src_duration);
            p.audio_file = Some(audio_rel.clone());
            self.save(p, ctx.sink)?;
        }
        if p.chunks.is_empty() {
            ctx.check()?;
            let target = self.settings.chunk_minutes as f64 * 60.0;
            let plan = if p.duration <= target + CUT_WINDOW_SECS {
                vec![(0.0, p.duration)]
            } else {
                ctx.progress(p, Stage::Prepare, 0.85, "尋找停頓點以便分段");
                let mut silences = self.ffmpeg.detect_silences(&audio_path, -35, 0.4, ctx.cancel).await?;
                if silences.len() < 3 {
                    // 背景噪音大的錄音放寬門檻
                    silences = self.ffmpeg.detect_silences(&audio_path, -28, 0.3, ctx.cancel).await?;
                }
                audio::plan_chunks(p.duration, &silences, target, CUT_WINDOW_SECS)
            };
            let mut chunks = Vec::with_capacity(plan.len());
            for (i, (start, end)) in plan.iter().copied().enumerate() {
                let file = if plan.len() == 1 {
                    audio_rel.clone()
                } else {
                    let rel = format!("chunks/chunk_{i:03}.{NORMALIZED_EXT}");
                    self.ffmpeg.extract(&audio_path, start, end, &dir.join(&rel)).await?;
                    rel
                };
                chunks.push(ChunkRecord {
                    index: i,
                    start,
                    end,
                    file,
                    stt: None,
                    attributed: false,
                });
            }
            ctx.log(
                p,
                LogLevel::Info,
                format!("全長 {}，分成 {} 段處理", fmt_clock(p.duration), chunks.len()),
            );
            p.chunks = chunks;
            self.save(p, ctx.sink)?;
        }
        ctx.progress(p, Stage::Prepare, 1.0, "前處理完成");
        Ok(())
    }

    // ---------- 轉錄 ----------

    /// 送出所有還沒轉錄的段落。供應商來不及處理（524）的段落對半切開再送一輪；
    /// 其他錯誤不中斷別段，等這一輪都回來、做完的存檔之後才回報。
    async fn transcribe(&self, p: &mut Project, ctx: &Ctx<'_>) -> Result<()> {
        loop {
            let (too_long, failed) = self.transcribe_pending(p, ctx).await?;
            ctx.check()?;
            // 有別的錯誤也先把太長的切好存檔，續跑時直接送短的
            if !too_long.is_empty() {
                for &i in too_long.iter().rev() {
                    self.split_chunk(p, i, ctx).await?;
                }
                for (k, c) in p.chunks.iter_mut().enumerate() {
                    c.index = k;
                }
                self.save(p, ctx.sink)?;
            }
            if let Some(e) = failed {
                return Err(e);
            }
            if too_long.is_empty() {
                break;
            }
        }
        let chars: usize = p
            .chunks
            .iter()
            .filter_map(|c| c.stt.as_ref())
            .map(|s| s.text.chars().count())
            .sum();
        if chars == 0 {
            return Err(Error::Other(
                "整段錄音沒有辨識到任何語音（請確認音量或語言設定）".into(),
            ));
        }
        ctx.progress(p, Stage::Transcribe, 1.0, "轉錄完成");
        Ok(())
    }

    /// 一輪轉錄，做完的段落逐段存檔。回傳太長、要切開重送的段落（由小到大）與第一個其他錯誤。
    async fn transcribe_pending(&self, p: &mut Project, ctx: &Ctx<'_>) -> Result<(Vec<usize>, Option<Error>)> {
        let pending: Vec<usize> = p.chunks.iter().filter(|c| c.stt.is_none()).map(|c| c.index).collect();
        let total = p.chunks.len();
        let mut too_long = Vec::new();
        let mut failed = None;
        if !pending.is_empty() {
            let dir = self.store.dir(p.id);
            let language = p.options.language.clone();
            let keyterms = p.options.keyterms.clone();
            let diarize = p.options.identify_speakers;
            let done_before = total - pending.len();
            ctx.progress(
                p,
                Stage::Transcribe,
                done_before as f32 / total as f32,
                format!("送出 {} 段給 {}", pending.len(), self.transcriber.model()),
            );
            let paths: Vec<(usize, std::path::PathBuf)> =
                pending.iter().map(|&i| (i, dir.join(&p.chunks[i].file))).collect();
            let jobs = paths.into_iter().map(|(i, path)| {
                let language = language.clone();
                let keyterms = keyterms.clone();
                async move {
                    let r = async {
                        let bytes = tokio::fs::read(&path).await?;
                        let req = TranscribeRequest {
                            audio: &bytes,
                            format: NORMALIZED_FORMAT,
                            language: language.as_deref(),
                            keyterms: &keyterms,
                            diarize,
                        };
                        self.transcriber.transcribe(req, ctx.cancel).await
                    };
                    (i, r.await)
                }
            });
            let mut stream = futures::stream::iter(jobs).buffer_unordered(self.settings.stt_concurrency.max(1));
            let mut done = done_before;
            while let Some((i, r)) = stream.next().await {
                let stt = match r {
                    Ok(stt) => stt,
                    Err(e) if e.is_gateway_timeout() && p.chunks[i].duration() > MIN_SPLIT_SECS => {
                        ctx.log(
                            p,
                            LogLevel::Warn,
                            format!(
                                "第 {}/{} 段（{}）太長，供應商來不及處理（{e}），切成兩段重送",
                                i + 1,
                                total,
                                fmt_clock(p.chunks[i].duration())
                            ),
                        );
                        too_long.push(i);
                        continue;
                    }
                    Err(e) => {
                        if !matches!(e, Error::Cancelled) {
                            ctx.log(p, LogLevel::Warn, format!("第 {}/{} 段轉錄失敗：{e}", i + 1, total));
                        }
                        failed.get_or_insert(e);
                        continue;
                    }
                };
                done += 1;
                p.usage.stt_usd += stt.cost;
                p.usage.stt_seconds += if stt.seconds > 0.0 {
                    stt.seconds
                } else {
                    p.chunks[i].duration()
                };
                ctx.log(
                    p,
                    LogLevel::Info,
                    format!("第 {}/{} 段轉錄完成（{} 字）", i + 1, total, stt.text.chars().count()),
                );
                p.chunks[i].stt = Some(stt);
                self.save(p, ctx.sink)?;
                ctx.progress(
                    p,
                    Stage::Transcribe,
                    done as f32 / total as f32,
                    format!("已完成 {done}/{total} 段"),
                );
            }
        }
        too_long.sort_unstable();
        Ok((too_long, failed))
    }

    /// 把第 `i` 段在中點附近的停頓切成兩段（換成兩筆新的 [`ChunkRecord`]，序號由呼叫端重排）。
    async fn split_chunk(&self, p: &mut Project, i: usize, ctx: &Ctx<'_>) -> Result<()> {
        let dir = self.store.dir(p.id);
        let old = p.chunks[i].clone();
        let src = dir.join(&old.file);
        let dur = old.duration();
        let silences = self
            .ffmpeg
            .detect_silences(&src, -35, 0.4, ctx.cancel)
            .await
            .unwrap_or_default();
        let plan = audio::plan_chunks(dur, &silences, dur / 2.0, (dur / 6.0).min(CUT_WINDOW_SECS));
        let cut = if plan.len() >= 2 { plan[0].1 } else { dur / 2.0 };
        let mut halves = Vec::with_capacity(2);
        for (from, to) in [(0.0, cut), (cut, dur)] {
            let (start, end) = (old.start + from, old.start + to);
            let rel = format!(
                "chunks/part_{:09}_{:09}.{NORMALIZED_EXT}",
                (start * 1000.0).round() as u64,
                (end * 1000.0).round() as u64
            );
            self.ffmpeg.extract(&src, from, to, &dir.join(&rel)).await?;
            halves.push(ChunkRecord {
                index: i,
                start,
                end,
                file: rel,
                stt: None,
                attributed: false,
            });
        }
        if old.file.starts_with("chunks/") {
            let _ = std::fs::remove_file(&src);
        }
        p.chunks.splice(i..=i, halves);
        Ok(())
    }

    // ---------- 說話者分配 ----------

    async fn build_segments(&self, p: &mut Project, ctx: &Ctx<'_>) -> Result<()> {
        let total = p.chunks.len();
        for i in 0..total {
            if p.chunks[i].attributed {
                continue;
            }
            ctx.check()?;
            ctx.progress(
                p,
                Stage::Speakers,
                i as f32 / total as f32,
                format!("第 {}/{} 段", i + 1, total),
            );
            p.segments.retain(|s| s.chunk != i);
            self.build_chunk(p, i, ctx).await?;
            p.sort_segments();
            p.chunks[i].attributed = true;
            self.save(p, ctx.sink)?;
        }
        ctx.progress(p, Stage::Speakers, 1.0, "說話者分配完成");
        Ok(())
    }

    async fn build_chunk(&self, p: &mut Project, i: usize, ctx: &Ctx<'_>) -> Result<()> {
        let chunk = p.chunks[i].clone();
        let Some(stt) = chunk.stt.clone() else { return Ok(()) };
        if stt.text.trim().is_empty() && stt.segments.is_empty() {
            return Ok(());
        }
        let identify = p.options.identify_speakers;

        // A. 供應商直接給了說話者（原生 diarize）
        if identify && stt.has_speaker_labels() {
            let turns: Vec<Turn> = if !stt.segments.is_empty() {
                stt.segments
                    .iter()
                    .map(|s| {
                        (
                            s.speaker.clone().unwrap_or_default(),
                            s.text.clone(),
                            Some((s.start, s.end)),
                        )
                    })
                    .collect()
            } else {
                group_words_by_speaker(&stt)
            };
            self.push_labelled_turns(p, &chunk, turns, Timing::Provider);
            return Ok(());
        }
        // B. 文字裡有 <spk:N> 標籤
        let tagged = parse_spk_tags(&stt.text);
        if identify && !tagged.is_empty() {
            let turns = tagged.into_iter().map(|(l, t)| (l, t, None)).collect();
            self.push_labelled_turns(p, &chunk, turns, Timing::Estimated);
            return Ok(());
        }
        let units = split_units(&stt.text, UNIT_MAX_CHARS);
        if units.is_empty() {
            return Ok(());
        }
        let word_times = times_from_words(&units, &stt.words);

        // C. 用能聽音訊的模型分配說話者
        if identify {
            match self.attribute_with_llm(p, &chunk, &units, ctx).await {
                Ok((speakers, anchors)) => {
                    let (times, timing) = match &word_times {
                        Some(t) => (t.clone(), Timing::Provider),
                        None => {
                            let w: Vec<usize> = units.iter().map(|u| weight(u)).collect();
                            (attribution::fill_times(&w, &anchors, chunk.duration()), Timing::Model)
                        }
                    };
                    self.push_units(p, &chunk, &units, &times, Some(&speakers), timing);
                    self.make_sample_clips(p, i, ctx).await;
                    return Ok(());
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => {
                    let msg = format!("第 {} 段說話者辨識失敗，這段先不標說話者：{e}", i + 1);
                    ctx.log(p, LogLevel::Warn, &msg);
                    p.warnings.push(msg);
                }
            }
        }
        // D. 不分說話者
        if !stt.segments.is_empty() && word_times.is_none() {
            let turns = stt
                .segments
                .iter()
                .map(|s| (String::new(), s.text.clone(), Some((s.start, s.end))))
                .collect();
            self.push_unlabelled(p, &chunk, turns);
        } else {
            let (times, timing) = match word_times {
                Some(t) => (t, Timing::Provider),
                None => (estimate_times(&units, 0.0, chunk.duration()), Timing::Estimated),
            };
            self.push_units(p, &chunk, &units, &times, None, timing);
        }
        Ok(())
    }

    /// 呼叫說話者分配模型（最多試 3 次），回傳每句的全域說話者 ID 與模型給的時間。
    async fn attribute_with_llm(
        &self,
        p: &mut Project,
        chunk: &ChunkRecord,
        units: &[String],
        ctx: &Ctx<'_>,
    ) -> Result<(Vec<String>, Vec<Option<f64>>)> {
        let dir = self.store.dir(p.id);
        let audio = Arc::new(tokio::fs::read(dir.join(&chunk.file)).await?);
        let stats = p.speaker_stats();
        let mut known = Vec::new();
        let mut clips_used = 0;
        for (id, _, _) in stats.iter().take(ROSTER_LIMIT) {
            let Some(sp) = p.speaker(id) else { continue };
            let clip = match (&sp.sample_clip, clips_used < self.settings.reference_clip_limit) {
                (Some(rel), true) => match tokio::fs::read(dir.join(rel)).await {
                    Ok(b) => {
                        clips_used += 1;
                        Some(Arc::new(b))
                    }
                    Err(_) => None,
                },
                _ => None,
            };
            let name = (sp.name_source != crate::model::NameSource::Label).then(|| sp.name.clone());
            known.push(KnownSpeaker {
                id: sp.id.clone(),
                name,
                note: sp.note.clone(),
                clip,
            });
        }
        let input = attribution::AttributionInput {
            model: &self.settings.attribution_model,
            audio,
            audio_format: NORMALIZED_FORMAT,
            chunk_index: chunk.index,
            chunk_count: p.chunks.len(),
            chunk_start: chunk.start,
            units,
            known: &known,
            known_total: p.speakers.len(),
            participants: &p.options.participants,
            expected_speakers: p.options.expected_speakers,
        };
        let request = attribution::build_request(&self.prompts, &input);
        let mut last_err = Error::Other("未執行".into());
        for attempt in 1..=3 {
            ctx.check()?;
            let resp = match self.attribution_llm.complete(request.clone(), ctx.cancel).await {
                Ok(r) => r,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => {
                    last_err = e;
                    continue;
                }
            };
            p.usage.llm_usd += resp.cost;
            p.usage.llm_input_tokens += resp.input_tokens;
            p.usage.llm_output_tokens += resp.output_tokens;
            let parsed = match attribution::parse_response(&resp.text) {
                Ok(x) => x,
                Err(e) => {
                    ctx.log(
                        p,
                        LogLevel::Warn,
                        format!("說話者分配第 {attempt} 次輸出無法解析，重試：{e}"),
                    );
                    last_err = e;
                    continue;
                }
            };
            let (raw_ids, anchors, covered) = attribution::resolve(&parsed, units.len());
            let coverage = covered as f64 / units.len() as f64;
            if coverage < 0.6 {
                last_err = Error::BadModelOutput(format!("只標了 {covered}/{} 句", units.len()));
                ctx.log(
                    p,
                    LogLevel::Warn,
                    format!("說話者分配第 {attempt} 次只涵蓋 {:.0}%，重試", coverage * 100.0),
                );
                continue;
            }
            if coverage < 0.98 {
                ctx.log(
                    p,
                    LogLevel::Info,
                    format!("模型漏標 {} 句，已沿用前一位說話者", units.len() - covered),
                );
            }
            let map = attribution::map_to_global(p, &parsed, &raw_ids);
            let speakers = raw_ids
                .iter()
                .map(|r| map.get(r).cloned().unwrap_or_else(|| r.clone()))
                .collect();
            return Ok((speakers, anchors));
        }
        Err(last_err)
    }

    fn push_units(
        &self,
        p: &mut Project,
        chunk: &ChunkRecord,
        units: &[String],
        times: &[(f64, f64)],
        speakers: Option<&[String]>,
        timing: Timing,
    ) {
        for (id, (k, text)) in (p.next_segment_id()..).zip(units.iter().enumerate()) {
            let (s, e) = times.get(k).copied().unwrap_or((0.0, chunk.duration()));
            p.segments.push(Segment {
                id,
                start: chunk.start + s,
                end: chunk.start + e.max(s),
                timing,
                speaker: speakers.and_then(|sp| sp.get(k).cloned()),
                text: text.clone(),
                chunk: chunk.index,
                edited: false,
            });
        }
    }

    /// 供應商或 `<spk:N>` 給的段內標籤：每段各自對應到新的全域說話者，跨段靠整合步驟合併。
    fn push_labelled_turns(&self, p: &mut Project, chunk: &ChunkRecord, turns: Vec<Turn>, timing: Timing) {
        let mut map: HashMap<String, String> = HashMap::new();
        let mut units = Vec::new();
        let mut speakers = Vec::new();
        let mut times: Vec<Option<(f64, f64)>> = Vec::new();
        for (label, text, t) in turns {
            let global = map
                .entry(label)
                .or_insert_with(|| p.add_speaker(None, String::new()))
                .clone();
            let pieces = split_units(&text, UNIT_MAX_CHARS);
            let est = t.map(|(s, e)| estimate_times(&pieces, s, e));
            for (k, piece) in pieces.into_iter().enumerate() {
                units.push(piece);
                speakers.push(global.clone());
                times.push(est.as_ref().map(|e| e[k]));
            }
        }
        let times = if times.iter().all(Option::is_some) {
            times.into_iter().map(|t| t.expect("checked")).collect()
        } else {
            estimate_times(&units, 0.0, chunk.duration())
        };
        let timing = if timing == Timing::Provider && times.len() == units.len() {
            timing
        } else {
            Timing::Estimated
        };
        self.push_units(p, chunk, &units, &times, Some(&speakers), timing);
    }

    fn push_unlabelled(&self, p: &mut Project, chunk: &ChunkRecord, turns: Vec<Turn>) {
        let units: Vec<String> = turns.iter().map(|t| t.1.clone()).collect();
        let times: Vec<(f64, f64)> = turns.iter().map(|t| t.2.unwrap_or((0.0, 0.0))).collect();
        self.push_units(p, chunk, &units, &times, None, Timing::Provider);
    }

    /// 這段新出現、還沒有聲音樣本的說話者，各剪一段 4～12 秒的樣本給後面段落比對。
    async fn make_sample_clips(&self, p: &mut Project, chunk_index: usize, ctx: &Ctx<'_>) {
        let Some(audio_rel) = p.audio_file.clone() else { return };
        let dir = self.store.dir(p.id);
        let need: Vec<String> = p
            .speakers
            .iter()
            .filter(|s| s.sample_clip.is_none())
            .map(|s| s.id.clone())
            .collect();
        for id in need {
            let best = p
                .segments
                .iter()
                .filter(|s| {
                    s.chunk == chunk_index && s.speaker.as_deref() == Some(&id) && s.timing != Timing::Estimated
                })
                .map(|s| (s.start, s.end))
                .filter(|(s, e)| e - s >= 2.5)
                .min_by(|a, b| ((a.1 - a.0) - 10.0).abs().total_cmp(&((b.1 - b.0) - 10.0).abs()));
            let Some((start, end)) = best else { continue };
            let rel = format!("clips/{id}.{NORMALIZED_EXT}");
            match self
                .ffmpeg
                .extract(&dir.join(&audio_rel), start, end.min(start + 12.0), &dir.join(&rel))
                .await
            {
                Ok(()) => {
                    if let Some(sp) = p.speaker_mut(&id) {
                        sp.sample_clip = Some(rel);
                    }
                }
                Err(e) => ctx.log(p, LogLevel::Warn, format!("剪 {id} 的聲音樣本失敗：{e}")),
            }
        }
    }

    // ---------- 整合 ----------

    async fn reconcile(&self, p: &mut Project, ctx: &Ctx<'_>) -> Result<()> {
        let worthwhile = p.speakers.len() > 1 && (p.chunks.len() > 1 || !p.options.participants.is_empty());
        if worthwhile {
            ctx.progress(p, Stage::Reconcile, 0.2, format!("比對 {} 位說話者", p.speakers.len()));
            let req = attribution::build_reconcile_request(&self.prompts, &self.settings.summary_model, p);
            match self.text_llm.complete(req, ctx.cancel).await {
                Ok(r) => {
                    p.usage.llm_usd += r.cost;
                    p.usage.llm_input_tokens += r.input_tokens;
                    p.usage.llm_output_tokens += r.output_tokens;
                    match attribution::parse_reconcile(&r.text) {
                        Ok(rec) => {
                            let (merged, named) = attribution::apply_reconciliation(p, &rec);
                            ctx.log(
                                p,
                                LogLevel::Info,
                                format!("說話者整合：合併 {merged} 組、命名 {named} 位"),
                            );
                        }
                        Err(e) => ctx.log(p, LogLevel::Warn, format!("說話者整合結果無法解析，略過：{e}")),
                    }
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => ctx.log(
                    p,
                    LogLevel::Warn,
                    format!("說話者整合失敗，略過（可在「說話者」頁手動合併）：{e}"),
                ),
            }
        }
        p.prune_unused_speakers();
        p.speakers_reconciled = true;
        self.save(p, ctx.sink)?;
        ctx.progress(p, Stage::Reconcile, 1.0, format!("共 {} 位說話者", p.speakers.len()));
        Ok(())
    }

    // ---------- 摘要 ----------

    async fn summarize_into(&self, p: &mut Project, instructions: &str, ctx: &Ctx<'_>) -> Result<()> {
        let pid = p.id;
        let sink = ctx.sink.clone();
        let report = move |f: f32, msg: String| {
            sink(Event::Progress {
                id: pid,
                stage: Stage::Summary,
                fraction: f,
                message: msg,
            });
        };
        let out = summary::summarize(
            p,
            self.text_llm.as_ref(),
            &self.settings,
            &self.prompts,
            instructions,
            &report,
            ctx.cancel,
        )
        .await?;
        p.usage.llm_usd += out.cost;
        p.usage.llm_input_tokens += out.input_tokens;
        p.usage.llm_output_tokens += out.output_tokens;
        p.summary = Some(out.summary);
        self.save(p, ctx.sink)?;
        Ok(())
    }

    /// 重新產生摘要（可帶新的指示）。
    pub async fn regenerate_summary(
        &self,
        mut p: Project,
        instructions: String,
        sink: EventSink,
        cancel: CancellationToken,
    ) -> Result<Project> {
        let ctx = Ctx {
            sink: &sink,
            cancel: &cancel,
        };
        p.models.summary = self.settings.summary_model.clone();
        self.summarize_into(&mut p, &instructions, &ctx).await?;
        Ok(p)
    }

    /// 清掉說話者結果，重新分配（逐字稿不重轉）。
    pub async fn reidentify_speakers(&self, mut p: Project, sink: EventSink, cancel: CancellationToken) -> Project {
        p.speakers.clear();
        p.segments.clear();
        p.speakers_reconciled = false;
        p.options.identify_speakers = true;
        for c in &mut p.chunks {
            c.attributed = false;
        }
        let clips = self.store.dir(p.id).join("clips");
        let _ = std::fs::remove_dir_all(&clips);
        self.run(p, sink, cancel).await
    }

    /// 預估費用（美元）：轉錄依秒計價；說話者與摘要粗估。
    pub fn estimate_cost(&self, duration_secs: f64) -> Option<f64> {
        self.stt_caps.as_ref()?.price_per_second.map(|p| p * duration_secs)
    }
}

/// 只有字詞層級有說話者時，把連續同一人的字詞併成一段。
fn group_words_by_speaker(stt: &SttResult) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for w in &stt.words {
        let sp = w.speaker.clone().unwrap_or_default();
        match out.last_mut() {
            Some(last) if last.0 == sp => {
                if crate::model::needs_space(&last.1, &w.text) {
                    last.1.push(' ');
                }
                last.1.push_str(&w.text);
                if let Some(t) = &mut last.2 {
                    t.1 = w.end;
                }
            }
            _ => out.push((sp, w.text.clone(), Some((w.start, w.end)))),
        }
    }
    out
}
