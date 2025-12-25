//! Quick tool to inspect diff data model for debugging scroll sync

use staged_lib::git::diff::get_file_diff;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() < 2 {
        eprintln!("Usage: inspect_diff <file_path> [staged]");
        eprintln!("  file_path: path to file relative to repo root");
        eprintln!("  staged: 'true' for staged diff, 'false' or omit for unstaged");
        std::process::exit(1);
    }
    
    let file_path = &args[1];
    let staged = args.get(2).map(|s| s == "true").unwrap_or(false);
    
    println!("Getting {} diff for: {}", if staged { "staged" } else { "unstaged" }, file_path);
    println!();
    
    match get_file_diff(None, file_path, staged) {
        Ok(diff) => {
            println!("Status: {}", diff.status);
            println!("Is binary: {}", diff.is_binary);
            println!();
            
            println!("=== BEFORE ({} lines) ===", diff.before.lines.len());
            println!("Path: {:?}", diff.before.path);
            for (i, line) in diff.before.lines.iter().enumerate() {
                let marker = match line.line_type.as_str() {
                    "removed" => "-",
                    "added" => "+",
                    _ => " ",
                };
                println!("[{:3}] {} {:4} | {}", i, marker, line.lineno, truncate(&line.content, 60));
            }
            println!();
            
            println!("=== AFTER ({} lines) ===", diff.after.lines.len());
            println!("Path: {:?}", diff.after.path);
            for (i, line) in diff.after.lines.iter().enumerate() {
                let marker = match line.line_type.as_str() {
                    "removed" => "-",
                    "added" => "+",
                    _ => " ",
                };
                println!("[{:3}] {} {:4} | {}", i, marker, line.lineno, truncate(&line.content, 60));
            }
            println!();
            
            println!("=== RANGES ({} total) ===", diff.ranges.len());
            for (i, range) in diff.ranges.iter().enumerate() {
                let kind = if range.changed { "CHANGE" } else { "context" };
                println!(
                    "[{:3}] {:7} | before: [{:3}, {:3}) ({:3} rows) | after: [{:3}, {:3}) ({:3} rows)",
                    i,
                    kind,
                    range.before.start,
                    range.before.end,
                    range.before.end - range.before.start,
                    range.after.start,
                    range.after.end,
                    range.after.end - range.after.start,
                );
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e.message);
            std::process::exit(1);
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
