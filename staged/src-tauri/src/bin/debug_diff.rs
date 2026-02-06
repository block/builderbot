//! Debug helper to inspect git diff hunks and alignments.
//!
//! Usage:
//!   cargo run --bin debug_diff -- <path> [ref1] [ref2]
//!
//! Examples:
//!   # Compare a file (HEAD vs working tree)
//!   cargo run --bin debug_diff -- src/main.rs
//!
//!   # Compare a file between two refs
//!   cargo run --bin debug_diff -- src/main.rs HEAD~3 HEAD

use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        print_usage();
        return;
    }

    run_git_mode(&args[1..]);
}

fn print_usage() {
    eprintln!(
        r#"Debug helper to inspect git diff hunks.

Usage:
  debug_diff <path> [ref1] [ref2]     Compare a file in git

Examples:
  debug_diff src/main.rs              # HEAD vs working tree
  debug_diff src/main.rs HEAD~3 HEAD  # Between two refs

This tool shows:
  1. The raw git diff output
  2. Parsed hunk information (what staged uses for alignments)
"#
    );
}

fn run_git_mode(args: &[String]) {
    if args.is_empty() {
        eprintln!("Error: Need a file path");
        print_usage();
        return;
    }

    let file_path = &args[0];
    let ref1 = args.get(1).map(|s| s.as_str()).unwrap_or("HEAD");
    let ref2 = args.get(2).map(|s| s.as_str());

    // Get before content
    let before = get_git_content(ref1, file_path);
    let before = match before {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error getting {file_path} at {ref1}: {e}");
            return;
        }
    };

    // Get after content
    let after = if let Some(r2) = ref2 {
        match get_git_content(r2, file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error getting {file_path} at {r2}: {e}");
                return;
            }
        }
    } else {
        // Working tree
        match std::fs::read_to_string(file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {file_path}: {e}");
                return;
            }
        }
    };

    // Run git diff
    println!("=== Git diff output ===");
    let git_args = if let Some(r2) = ref2 {
        vec!["diff", "--no-color", ref1, r2, "--", file_path]
    } else {
        vec!["diff", "--no-color", ref1, "--", file_path]
    };

    let git_output = Command::new("git").args(&git_args).output();

    match git_output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.is_empty() {
                println!("(no differences)");
                return;
            } else {
                println!("{stdout}");
            }
        }
        Err(e) => {
            println!("(git diff failed: {e})");
            return;
        }
    }

    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();

    println!("\n=== File stats ===");
    println!("Before: {} lines", before_lines.len());
    println!("After:  {} lines", after_lines.len());

    // Parse hunks from git diff output
    println!("\n=== Parsed hunks (what staged uses) ===");
    let hunks = parse_hunks_from_git(file_path, ref1, ref2);
    if hunks.is_empty() {
        println!("  (no hunks)");
    } else {
        for (i, hunk) in hunks.iter().enumerate() {
            println!(
                "  {}. old[{}..{}] ({} lines) -> new[{}..{}] ({} lines)",
                i + 1,
                hunk.0,
                hunk.0 + hunk.1,
                hunk.1,
                hunk.2,
                hunk.2 + hunk.3,
                hunk.3
            );
        }
    }

    println!("\n=== Expected alignments ===");
    let alignments = compute_alignments(&hunks, before_lines.len(), after_lines.len());
    for (i, a) in alignments.iter().enumerate() {
        let kind = if a.4 { "CHANGED" } else { "unchanged" };
        println!(
            "  {}. {} before[{}..{}] <-> after[{}..{}]",
            i + 1,
            kind,
            a.0,
            a.1,
            a.2,
            a.3
        );
    }
}

fn get_git_content(refspec: &str, path: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(["show", &format!("{refspec}:{path}")])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Parse hunks from git diff output
/// Returns: Vec<(old_start, old_lines, new_start, new_lines)> - all 0-indexed
fn parse_hunks_from_git(
    file_path: &str,
    ref1: &str,
    ref2: Option<&str>,
) -> Vec<(u32, u32, u32, u32)> {
    let git_args = if let Some(r2) = ref2 {
        vec!["diff", "--no-color", ref1, r2, "--", file_path]
    } else {
        vec!["diff", "--no-color", ref1, "--", file_path]
    };

    let output = match Command::new("git").args(&git_args).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut hunks = Vec::new();

    for line in stdout.lines() {
        if line.starts_with("@@") {
            if let Some(hunk) = parse_hunk_header(line) {
                hunks.push(hunk);
            }
        }
    }

    hunks
}

/// Parse a hunk header like "@@ -1,3 +1,4 @@" into (old_start, old_lines, new_start, new_lines)
/// Returns 0-indexed values
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    // Format: @@ -old_start,old_lines +new_start,new_lines @@
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let old_part = parts[1].trim_start_matches('-');
    let new_part = parts[2].trim_start_matches('+');

    let (old_start, old_lines) = parse_range(old_part)?;
    let (new_start, new_lines) = parse_range(new_part)?;

    // Convert from 1-indexed to 0-indexed
    let old_start = if old_start == 0 { 0 } else { old_start - 1 };
    let new_start = if new_start == 0 { 0 } else { new_start - 1 };

    Some((old_start, old_lines, new_start, new_lines))
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = s.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        // Single line: "5" means start=5, count=1
        Some((s.parse().ok()?, 1))
    }
}

/// Compute alignments from hunks (mirrors the Rust implementation)
/// Returns: Vec<(before_start, before_end, after_start, after_end, changed)>
fn compute_alignments(
    hunks: &[(u32, u32, u32, u32)],
    before_len: usize,
    after_len: usize,
) -> Vec<(u32, u32, u32, u32, bool)> {
    let before_len = before_len as u32;
    let after_len = after_len as u32;

    if hunks.is_empty() {
        if before_len == 0 && after_len == 0 {
            return vec![];
        }
        // All added or all deleted
        return vec![(0, before_len, 0, after_len, true)];
    }

    let mut alignments = Vec::new();
    let mut before_pos = 0u32;
    let mut after_pos = 0u32;

    for &(old_start, old_lines, new_start, new_lines) in hunks {
        // Unchanged region before this hunk
        if before_pos < old_start || after_pos < new_start {
            alignments.push((before_pos, old_start, after_pos, new_start, false));
        }

        // The hunk itself
        let hunk_before_end = old_start + old_lines;
        let hunk_after_end = new_start + new_lines;
        alignments.push((old_start, hunk_before_end, new_start, hunk_after_end, true));

        before_pos = hunk_before_end;
        after_pos = hunk_after_end;
    }

    // Unchanged region after the last hunk
    if before_pos < before_len || after_pos < after_len {
        alignments.push((before_pos, before_len, after_pos, after_len, false));
    }

    alignments
}
