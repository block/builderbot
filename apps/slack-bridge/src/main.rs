//! slack-bridge entry point.
//!
//! Commands:
//! - `run [--dry-run]` — connect to Slack via Socket Mode and serve mentions.
//!   With `--dry-run`, mentions are echoed back with a canned progress sequence
//!   instead of invoking an agent (validates the Slack round-trip).
//! - `dev <fixture.json>` — fully offline: run a captured event through the
//!   parser + progress formatter and print the result. No tokens, no network.

use anyhow::{Context, Result};

use slack_bridge::config::Config;
use slack_bridge::events::parse_app_mention;
use slack_bridge::progress::ProgressState;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("run") => {
            let dry_run = args.any(|a| a == "--dry-run");
            run(dry_run)
        }
        Some("dev") => {
            let path = args
                .next()
                .context("usage: slack-bridge dev <event-fixture.json>")?;
            run_dev(&path)
        }
        Some(other) => {
            anyhow::bail!(
                "unknown command '{other}'. Available: run [--dry-run], dev <fixture.json>"
            )
        }
        None => {
            eprintln!("usage:");
            eprintln!("  slack-bridge run [--dry-run]   # live Socket Mode bridge");
            eprintln!("  slack-bridge dev <fixture.json>  # offline parse/format check");
            Ok(())
        }
    }
}

/// Live Socket Mode bridge.
fn run(dry_run: bool) -> Result<()> {
    let config = Config::from_env()?;
    eprintln!(
        "slack-bridge: starting (agent={}, workdir={}, dry_run={})",
        config.agent,
        config.workdir.display(),
        dry_run
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    rt.block_on(slack_bridge::slack::run(config, dry_run))
}

/// Offline dry-run: parse a fixture event and print the rendered progress
/// messages the bridge would post, without contacting Slack or an agent.
fn run_dev(path: &str) -> Result<()> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read fixture '{path}'"))?;
    let payload: serde_json::Value =
        serde_json::from_str(&raw).context("fixture is not valid JSON")?;

    // Accept either a bare event or a Socket Mode envelope payload wrapping one.
    let event = payload
        .get("payload")
        .and_then(|p| p.get("event"))
        .or_else(|| payload.get("event"))
        .unwrap_or(&payload);

    let Some(task) = parse_app_mention(event) else {
        println!("(event ignored — not an actionable @builderbot mention)");
        return Ok(());
    };

    println!("Parsed task:");
    println!("  channel:   {}", task.channel);
    println!("  thread_ts: {}", task.thread_ts);
    println!(
        "  user:      {}",
        task.user.as_deref().unwrap_or("(unknown)")
    );
    println!("  prompt:    {}", task.prompt);
    println!();

    // Simulate a short progress stream so the formatter output is visible.
    let mut state = ProgressState::new();
    println!("--- initial post ---\n{}\n", state.render());

    state.record_tool_call("cargo test --workspace");
    state.append_text("Running the test suite to reproduce the issue.");
    println!("--- progress update ---\n{}\n", state.render());

    state.record_tool_result("test result: ok. 42 passed; 0 failed");
    state.append_text("\n\nAll tests pass. Nothing further to do.");
    state.mark_done();
    println!("--- final update ---\n{}", state.render());

    Ok(())
}
