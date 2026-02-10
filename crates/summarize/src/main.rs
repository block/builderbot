mod auth;
mod files;
mod llm;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// Portable CLI tool to summarize files using an LLM.
///
/// Reads files deterministically into a context window with a prompt,
/// and returns an LLM-generated summary.
#[derive(Parser, Debug)]
#[command(name = "summarize", version, about)]
struct Cli {
    /// Inline prompt text (mutually exclusive with --prompt-file)
    #[arg(long = "prompt", short = 'p', group = "prompt_source")]
    prompt: Option<String>,

    /// Path to a file containing the prompt
    #[arg(long = "prompt-file", short = 'f', group = "prompt_source")]
    prompt_file: Option<PathBuf>,

    /// File extensions to include (e.g. --ext rs --ext py). If omitted, all files included.
    #[arg(long = "ext", short = 'e')]
    ext: Vec<String>,

    /// LLM model name (overrides SUMMARIZE_MODEL env var)
    #[arg(long)]
    model: Option<String>,

    /// Output format: text (default) or json
    #[arg(long, default_value = "text")]
    output: OutputFormat,

    /// Files or directories to include. Directories are expanded recursively.
    #[arg(required = true)]
    paths: Vec<String>,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Resolve prompt
    let prompt = match (&cli.prompt, &cli.prompt_file) {
        (Some(text), _) => text.clone(),
        (_, Some(path)) => std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read prompt file '{}': {}", path.display(), e))?,
        (None, None) => anyhow::bail!("Must provide either --prompt or --prompt-file"),
    };

    // Collect files
    let working_dir = std::env::current_dir()?;
    let extensions = if cli.ext.is_empty() { None } else { Some(cli.ext) };
    let collected = files::collect_files(&cli.paths, &working_dir, &extensions)
        .map_err(|e| anyhow::anyhow!(e))?;

    if collected.is_empty() {
        anyhow::bail!("No files found matching the specified paths and extensions.");
    }

    let total_lines: usize = collected.iter().map(|f| f.lines).sum();
    let file_count = collected.len();

    eprintln!(
        "Collected {} files ({} lines)",
        file_count, total_lines
    );

    // Build the full prompt with file contents
    let full_prompt = files::build_prompt(&collected, &prompt, &working_dir);

    // Resolve model
    let model = cli
        .model
        .or_else(|| std::env::var("SUMMARIZE_MODEL").ok())
        .unwrap_or_else(|| "databricks-claude-sonnet-4".to_string());

    // Call LLM
    let response = llm::complete(&model, &full_prompt).await?;

    // Output
    match cli.output {
        OutputFormat::Text => {
            println!("{}", response);
            eprintln!("\n---\nAnalyzed {} files ({} lines)", file_count, total_lines);
        }
        OutputFormat::Json => {
            let output = serde_json::json!({
                "summary": response,
                "files": file_count,
                "lines": total_lines,
                "model": model,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(())
}
