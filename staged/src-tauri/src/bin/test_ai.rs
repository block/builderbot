//! Test binary for AI diff analysis.
//!
//! Usage:
//!   cargo run --bin test_ai -- <base>..<head>         # Real diff (current dir)
//!   cargo run --bin test_ai -- <base>..<head> <repo>  # Real diff (specific repo)
//!
//! Examples:
//!   cargo run --bin test_ai -- HEAD~1..HEAD
//!   cargo run --bin test_ai -- main..HEAD ./my-repo

use staged_lib::{ai, git};
use std::env;
use std::path::Path;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    println!("=== Smart Diff AI Test ===\n");

    // Check for AI agent
    match ai::find_acp_agent() {
        Some(agent) => println!("✓ Found AI agent: {}\n", agent.name()),
        None => {
            eprintln!("✗ No AI agent found. Install goose: https://github.com/block/goose");
            std::process::exit(1);
        }
    }

    match args.first().map(|s| s.as_str()) {
        None | Some("--help") | Some("-h") => print_help(),
        Some(range) => {
            let repo_path = args.get(1).map(|s| s.as_str()).unwrap_or(".");
            test_real_diff(range, repo_path).await;
        }
    }
}

fn print_help() {
    println!(
        r#"Usage:
  cargo run --bin test_ai -- <base>..<head>         # Real diff (current dir)
  cargo run --bin test_ai -- <base>..<head> <repo>  # Real diff (specific repo)

Examples:
  cargo run --bin test_ai -- HEAD~1..HEAD           # Last commit
  cargo run --bin test_ai -- HEAD~3..HEAD           # Last 3 commits
  cargo run --bin test_ai -- main..HEAD             # Changes since main
  cargo run --bin test_ai -- main..feature ~/repo   # Branch diff in specific repo
"#
    );
}

async fn test_real_diff(range: &str, repo_path: &str) {
    // Parse base..head
    let parts: Vec<&str> = range.split("..").collect();
    if parts.len() != 2 {
        eprintln!("✗ Invalid range format. Use: base..head (e.g., HEAD~1..HEAD)");
        std::process::exit(1);
    }
    let (base, head) = (parts[0], parts[1]);

    let repo = Path::new(repo_path);
    println!(
        "Repository: {}",
        repo.canonicalize().unwrap_or(repo.to_path_buf()).display()
    );
    println!("Diff range: {base}..{head}\n");

    // Build DiffSpec
    let spec = git::DiffSpec {
        base: git::GitRef::Rev(base.to_string()),
        head: git::GitRef::Rev(head.to_string()),
    };

    // Run analysis - the backend handles file listing and content loading
    println!("Analyzing diff with AI via ACP (this may take a few seconds)...\n");

    match ai::analysis::analyze_diff(repo, &spec, None).await {
        Ok(result) => {
            println!("═══════════════════════════════════════════════════════════════");
            println!("                     CHANGESET ANALYSIS");
            println!("═══════════════════════════════════════════════════════════════\n");
            println!("{}\n", result.summary);

            println!("Key Changes:");
            for change in &result.key_changes {
                println!("  • {change}");
            }

            if !result.concerns.is_empty() {
                println!("\nConcerns:");
                for concern in &result.concerns {
                    println!("  ⚠ {concern}");
                }
            }

            println!("\n───────────────────────────────────────────────────────────────");
            println!("                     FILE ANNOTATIONS");
            println!("───────────────────────────────────────────────────────────────\n");

            for (path, annotations) in &result.file_annotations {
                if annotations.is_empty() {
                    println!("{path}: (no annotations)");
                } else {
                    println!("{path}:");
                    for ann in annotations {
                        let span_info = match (&ann.before_span, &ann.after_span) {
                            (Some(b), Some(a)) => {
                                format!("before {}-{}, after {}-{}", b.start, b.end, a.start, a.end)
                            }
                            (None, Some(a)) => format!("lines {}-{}", a.start, a.end),
                            (Some(b), None) => format!("before {}-{}", b.start, b.end),
                            (None, None) => "general".to_string(),
                        };
                        println!("\n  [{:?}] ({}) {}", ann.category, span_info, ann.id);
                        if let Some(ref desc) = ann.before_description {
                            println!("    Was: {desc}");
                        }
                        println!("    {}", ann.content);
                    }
                }
                println!();
            }
        }
        Err(e) => {
            eprintln!("✗ Analysis failed: {e}");
            std::process::exit(1);
        }
    }
}
