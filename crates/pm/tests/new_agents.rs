use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn pm() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pm"))
}

fn assert_success(output: std::process::Output) {
    assert!(
        output.status.success(),
        "expected command to succeed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn assert_symlink_to_agents(project_dir: &Path) {
    let claude_md = project_dir.join("CLAUDE.md");
    let target = fs::read_link(&claude_md).expect("CLAUDE.md should be a symlink");
    assert_eq!(target, Path::new("AGENTS.md"));
}

#[test]
fn new_without_agents_md_option_skips_agent_files() {
    let workspace = TempDir::new().unwrap();

    let output = pm()
        .arg("new")
        .arg("feature")
        .current_dir(workspace.path())
        .output()
        .unwrap();

    assert_success(output);
    let project_dir = workspace.path().join("feature");
    assert!(!project_dir.join("AGENTS.md").exists());
    assert!(!project_dir.join("CLAUDE.md").exists());
}

#[test]
fn new_with_agents_md_path_copies_agents_and_links_claude() {
    let workspace = TempDir::new().unwrap();
    let source_dir = TempDir::new().unwrap();
    let source = source_dir.path().join("source.AGENTS.md");
    fs::write(&source, "# Agent instructions\n").unwrap();

    let output = pm()
        .arg("new")
        .arg("feature")
        .arg(format!("--agents-md={}", source.display()))
        .current_dir(workspace.path())
        .output()
        .unwrap();

    assert_success(output);
    let project_dir = workspace.path().join("feature");
    assert_eq!(
        fs::read_to_string(project_dir.join("AGENTS.md")).unwrap(),
        "# Agent instructions\n"
    );
    assert_symlink_to_agents(&project_dir);
}

#[test]
fn new_with_agents_md_without_path_uses_cwd_agents_md() {
    let workspace = TempDir::new().unwrap();
    fs::write(workspace.path().join("AGENTS.md"), "# Local agents\n").unwrap();

    let output = pm()
        .arg("new")
        .arg("feature")
        .arg("--agents-md")
        .current_dir(workspace.path())
        .output()
        .unwrap();

    assert_success(output);
    let project_dir = workspace.path().join("feature");
    assert_eq!(
        fs::read_to_string(project_dir.join("AGENTS.md")).unwrap(),
        "# Local agents\n"
    );
    assert_symlink_to_agents(&project_dir);
}

#[test]
fn new_with_agents_md_before_name_uses_cwd_agents_md() {
    let workspace = TempDir::new().unwrap();
    fs::write(workspace.path().join("AGENTS.md"), "# Local agents\n").unwrap();

    let output = pm()
        .arg("new")
        .arg("--agents-md")
        .arg("feature")
        .current_dir(workspace.path())
        .output()
        .unwrap();

    assert_success(output);
    let project_dir = workspace.path().join("feature");
    assert_eq!(
        fs::read_to_string(project_dir.join("AGENTS.md")).unwrap(),
        "# Local agents\n"
    );
    assert_symlink_to_agents(&project_dir);
}

#[test]
fn new_with_agents_md_source_matching_destination_preserves_agents_md() {
    let workspace = TempDir::new().unwrap();
    let project_dir = workspace.path().join("feature");
    fs::create_dir(&project_dir).unwrap();
    fs::write(project_dir.join("AGENTS.md"), "# Existing agents\n").unwrap();

    let output = pm()
        .arg("new")
        .arg("feature")
        .arg("--agents-md=feature/AGENTS.md")
        .current_dir(workspace.path())
        .output()
        .unwrap();

    assert_success(output);
    assert_eq!(
        fs::read_to_string(project_dir.join("AGENTS.md")).unwrap(),
        "# Existing agents\n"
    );
    assert_symlink_to_agents(&project_dir);
}

#[test]
fn new_with_agents_md_without_path_fails_when_cwd_agents_md_is_missing() {
    let workspace = TempDir::new().unwrap();

    let output = pm()
        .arg("new")
        .arg("feature")
        .arg("--agents-md")
        .current_dir(workspace.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AGENTS.md"),
        "stderr should mention AGENTS.md, got:\n{stderr}"
    );
    assert!(
        stderr.contains("--agents-md"),
        "stderr should ask for --agents-md, got:\n{stderr}"
    );
}
