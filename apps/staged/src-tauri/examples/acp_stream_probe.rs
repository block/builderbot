use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use acp_client::{AcpDriver, AgentDriver, MessageWriter, Store};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

struct NoopStore;

#[async_trait]
impl Store for NoopStore {
    fn set_agent_session_id(&self, session_id: &str, agent_session_id: &str) -> Result<(), String> {
        println!(
            "[probe] set_agent_session_id: session_id={} agent_session_id={}",
            session_id, agent_session_id
        );
        Ok(())
    }
}

#[derive(Default)]
struct ProbeState {
    tool_call_counts: HashMap<String, usize>,
    total_tool_calls: usize,
    total_tool_title_updates: usize,
    total_tool_results: usize,
    assistant_chunks: usize,
}

struct ProbeWriter {
    state: Arc<Mutex<ProbeState>>,
}

impl ProbeWriter {
    fn new(state: Arc<Mutex<ProbeState>>) -> Self {
        Self { state }
    }

    fn truncate(s: &str) -> String {
        const MAX: usize = 180;
        let mut out = String::new();
        for (idx, ch) in s.chars().enumerate() {
            if idx >= MAX {
                out.push_str("...");
                break;
            }
            out.push(ch);
        }
        out.replace('\n', "\\n")
    }
}

#[async_trait]
impl MessageWriter for ProbeWriter {
    async fn append_text(&self, text: &str) {
        let mut state = self.state.lock().expect("probe state lock poisoned");
        state.assistant_chunks += 1;
        println!(
            "[probe] assistant_chunk #{}: {}",
            state.assistant_chunks,
            Self::truncate(text)
        );
    }

    async fn finalize(&self) {
        println!("[probe] finalize");
    }

    async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        _raw_input: Option<&serde_json::Value>,
    ) {
        let mut state = self.state.lock().expect("probe state lock poisoned");
        state.total_tool_calls += 1;
        let count_for_id = *state
            .tool_call_counts
            .entry(tool_call_id.to_string())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        println!(
            "[probe] tool_call #{} id={} seen_for_id={} title={}",
            state.total_tool_calls,
            tool_call_id,
            count_for_id,
            Self::truncate(title)
        );
    }

    async fn update_tool_call_title(
        &self,
        tool_call_id: &str,
        title: Option<&str>,
        _raw_input: Option<&serde_json::Value>,
    ) {
        let mut state = self.state.lock().expect("probe state lock poisoned");
        state.total_tool_title_updates += 1;
        let title = title.unwrap_or("<none>");
        println!(
            "[probe] tool_call_update #{} id={} title={}",
            state.total_tool_title_updates,
            tool_call_id,
            Self::truncate(title)
        );
    }

    async fn record_tool_result(&self, tool_call_id: &str, content: &str) {
        let mut state = self.state.lock().expect("probe state lock poisoned");
        state.total_tool_results += 1;
        println!(
            "[probe] tool_result #{} id={}: {}",
            state.total_tool_results,
            tool_call_id,
            Self::truncate(content)
        );
    }
}

fn print_usage() {
    eprintln!(
        "Usage:
  cargo run --manifest-path src-tauri/Cargo.toml --example acp_stream_probe -- \\
    --provider <provider-id> --workdir <path> --prompt <text>

Defaults:
  --provider codex
  --workdir .
  --prompt \"Run `echo hello` and summarize the output in one sentence.\""
    );
}

fn parse_args() -> Result<(String, PathBuf, String)> {
    let mut provider = "codex".to_string();
    let mut workdir = PathBuf::from(".");
    let mut prompt = "Run `echo hello` and summarize the output in one sentence.".to_string();

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--provider" => {
                provider = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --provider"))?;
            }
            "--workdir" => {
                workdir = PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("missing value for --workdir"))?,
                );
            }
            "--prompt" => {
                prompt = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --prompt"))?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(anyhow!("unknown argument: {other}"));
            }
        }
    }

    Ok((provider, workdir, prompt))
}

fn main() -> Result<()> {
    let (provider, workdir, prompt) = parse_args()?;
    println!("[probe] provider={provider} workdir={}", workdir.display());
    println!("[probe] prompt={}", ProbeWriter::truncate(&prompt));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();

    local.block_on(&rt, async move {
        let driver = AcpDriver::new(&provider).map_err(|e| anyhow!(e))?;
        let store = Arc::new(NoopStore) as Arc<dyn Store>;
        let state = Arc::new(Mutex::new(ProbeState::default()));
        let writer = Arc::new(ProbeWriter::new(Arc::clone(&state))) as Arc<dyn MessageWriter>;
        let cancel_token = CancellationToken::new();

        let result = driver
            .run(
                "probe-session",
                &prompt,
                &[],
                &workdir,
                &store,
                &writer,
                &cancel_token,
                None,
                &[],
            )
            .await;

        let state = state.lock().expect("probe state lock poisoned");
        println!("[probe] result={result:?}");
        println!(
            "[probe] summary: assistant_chunks={} tool_calls={} tool_call_updates={} tool_results={}",
            state.assistant_chunks,
            state.total_tool_calls,
            state.total_tool_title_updates,
            state.total_tool_results
        );

        let mut duplicate_ids: Vec<(&String, &usize)> = state
            .tool_call_counts
            .iter()
            .filter(|(_, count)| **count > 1)
            .collect();
        duplicate_ids.sort_by(|(a, _), (b, _)| a.cmp(b));

        if duplicate_ids.is_empty() {
            println!("[probe] duplicate_tool_call_ids=none");
        } else {
            println!("[probe] duplicate_tool_call_ids:");
            for (id, count) in duplicate_ids {
                println!("  - id={id} count={count}");
            }
        }

        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}
