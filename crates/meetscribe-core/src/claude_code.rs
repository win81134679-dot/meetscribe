//! 用使用者電腦上的 Claude Code CLI（`claude -p`）做純文字工作（摘要、說話者整合）。
//!
//! 走使用者自己的 Claude 登入（訂閱或 API Key），不經 OpenRouter。
//! 隔離做法：工作目錄是程式資料夾裡的空資料夾、`--setting-sources project`（不載入使用者全域
//! hook 與 plugin）、`--tools ""`（不給任何工具）、`--strict-mcp-config`、不存 session。
//! 不用 `--bare`：它只認 ANTHROPIC_API_KEY，訂閱登入的人會失敗。

use crate::config::Settings;
use crate::providers::{ContentPart, LlmClient, LlmRequest, LlmResponse, Role};
use crate::{Error, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub struct ClaudeCodeCli {
    program: PathBuf,
    workdir: PathBuf,
    timeout: Duration,
}

impl ClaudeCodeCli {
    /// `workdir` 是給 CLI 當工作目錄的空資料夾（不存在會建立）。
    pub fn new(settings: &Settings, workdir: PathBuf) -> Result<Self> {
        let program = locate(&settings.claude_cli_path).ok_or_else(|| {
            Error::Other(
                "找不到 Claude Code CLI（claude）。請先安裝並登入 Claude Code，或在設定指定 claude.exe 的位置".into(),
            )
        })?;
        std::fs::create_dir_all(&workdir)?;
        Ok(Self {
            program,
            workdir,
            timeout: Duration::from_secs(settings.request_timeout_secs),
        })
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// `claude --version`（設定頁測試用）。
    pub async fn version(&self) -> Result<String> {
        let out = command(&self.program).arg("--version").output().await?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// 設定值 → PATH → `%USERPROFILE%\.local\bin`（官方安裝程式的位置）。
pub fn locate(custom: &str) -> Option<PathBuf> {
    let custom = custom.trim().trim_matches('"');
    if !custom.is_empty() {
        let p = PathBuf::from(custom);
        return p.is_file().then_some(p);
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        dirs.push(PathBuf::from(home).join(".local").join("bin"));
    }
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

fn command(program: &Path) -> tokio::process::Command {
    // .cmd 包裝要經過 cmd.exe
    let mut c = if program.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd")) {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        tokio::process::Command::new(program)
    };
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.kill_on_drop(true);
    c
}

/// 解析 `--output-format json` 的結果。
pub fn parse_output(stdout: &str) -> Result<LlmResponse> {
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        Error::BadModelOutput(format!(
            "Claude Code 輸出不是 JSON：{}",
            stdout.chars().take(200).collect::<String>()
        ))
    })?;
    let text = v["result"].as_str().unwrap_or_default().to_string();
    if v["is_error"].as_bool().unwrap_or(false) {
        return Err(Error::Other(format!(
            "Claude Code 回報錯誤：{}",
            if text.is_empty() {
                v["subtype"].as_str().unwrap_or("未知")
            } else {
                &text
            }
        )));
    }
    if text.trim().is_empty() {
        return Err(Error::BadModelOutput("Claude Code 回傳空白內容".into()));
    }
    let u = &v["usage"];
    let input = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
        .iter()
        .filter_map(|k| u[*k].as_u64())
        .sum();
    Ok(LlmResponse {
        text,
        // 訂閱登入時 total_cost_usd 只是牌價換算，不是實際扣款，不計入費用
        cost: 0.0,
        input_tokens: input,
        output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
        finish_reason: v["stop_reason"].as_str().map(|r| {
            if r == "max_tokens" {
                "length".into()
            } else {
                r.to_string()
            }
        }),
    })
}

#[async_trait]
impl LlmClient for ClaudeCodeCli {
    async fn complete(&self, req: LlmRequest, cancel: &CancellationToken) -> Result<LlmResponse> {
        let mut system = String::new();
        let mut prompt = String::new();
        for m in &req.messages {
            for part in &m.content {
                match part {
                    ContentPart::Text(t) if m.role == Role::System => {
                        system.push_str(t);
                        system.push('\n');
                    }
                    ContentPart::Text(t) => {
                        prompt.push_str(t);
                        prompt.push('\n');
                    }
                    ContentPart::Audio { .. } => {
                        return Err(Error::Other(
                            "Claude Code CLI 不能聽音訊，說話者分配請改用 Gemini 或 OpenRouter".into(),
                        ));
                    }
                }
            }
        }
        let mut cmd = command(&self.program);
        cmd.current_dir(&self.workdir)
            .args([
                "-p",
                "--output-format",
                "json",
                "--tools",
                "",
                "--strict-mcp-config",
                "--no-session-persistence",
            ])
            .args(["--setting-sources", "project"]);
        if !system.trim().is_empty() {
            cmd.arg("--system-prompt").arg(system.trim());
        }
        if !req.model.trim().is_empty() {
            cmd.arg("--model").arg(req.model.trim());
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Other(format!("無法啟動 Claude Code CLI：{e}")))?;
        let mut stdin = child.stdin.take().expect("piped");
        let input = prompt.into_bytes();
        let writer = tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
            let _ = stdin.shutdown().await;
        });
        let output = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            _ = tokio::time::sleep(self.timeout) => return Err(Error::Other("Claude Code CLI 逾時".into())),
            o = child.wait_with_output() => o?,
        };
        let _ = writer.await;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() && stdout.trim().is_empty() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Other(format!(
                "Claude Code CLI 失敗（{}）：{}",
                output.status,
                err.lines().rev().take(3).collect::<Vec<_>>().join(" | ")
            )));
        }
        parse_output(&stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_result_json() {
        let r = parse_output(
            r##"{"type":"result","subtype":"success","is_error":false,"result":"# 摘要","stop_reason":"end_turn",
               "total_cost_usd":0.06,"usage":{"input_tokens":2,"cache_creation_input_tokens":100,"output_tokens":7}}"##,
        )
        .unwrap();
        assert_eq!(r.text, "# 摘要");
        assert_eq!((r.input_tokens, r.output_tokens), (102, 7));
        assert_eq!(r.cost, 0.0);
        assert!(parse_output(r#"{"is_error":true,"result":"Not logged in"}"#).is_err());
        assert!(parse_output("not json").is_err());
    }
}
