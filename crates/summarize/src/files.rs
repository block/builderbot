use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};

pub struct FileContent {
    pub path: PathBuf,
    pub content: String,
    pub lines: usize,
}

/// Build a gitignore matcher from the working directory.
fn build_gitignore(working_dir: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(working_dir);

    let gitignore_path = working_dir.join(".gitignore");
    if gitignore_path.is_file() {
        let _ = builder.add(&gitignore_path);
    }

    // Always ignore .git directory
    let _ = builder.add_line(None, ".git/");

    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

/// Check if a file matches the extension filter.
fn should_include_file(path: &Path, extensions: &Option<Vec<String>>) -> bool {
    match extensions {
        Some(exts) => {
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_lowercase(),
                None => return false,
            };
            exts.iter().any(|e| e.to_lowercase() == ext)
        }
        None => true,
    }
}

/// Collect files from the given paths, expanding directories recursively.
/// Respects .gitignore and optional extension filters.
pub fn collect_files(
    paths: &[String],
    working_dir: &Path,
    extensions: &Option<Vec<String>>,
) -> Result<Vec<FileContent>, String> {
    let gitignore = build_gitignore(working_dir);
    let mut files = Vec::new();

    for path_str in paths {
        let path = if Path::new(path_str).is_absolute() {
            PathBuf::from(path_str)
        } else {
            working_dir.join(path_str)
        };

        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()));
        }

        if path.is_dir() {
            collect_from_dir(&path, &mut files, extensions, &gitignore)?;
        } else if path.is_file() && should_include_file(&path, extensions) {
            collect_file(&path, &mut files)?;
        }
    }

    Ok(files)
}

fn collect_from_dir(
    dir: &Path,
    files: &mut Vec<FileContent>,
    extensions: &Option<Vec<String>>,
    gitignore: &Gitignore,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();

        if gitignore.matched(&path, path.is_dir()).is_ignore() {
            continue;
        }

        if path.is_dir() {
            collect_from_dir(&path, files, extensions, gitignore)?;
        } else if path.is_file() && should_include_file(&path, extensions) {
            collect_file(&path, files)?;
        }
    }

    Ok(())
}

fn collect_file(path: &Path, files: &mut Vec<FileContent>) -> Result<(), String> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            // Skip binary / unreadable files silently
            return Ok(());
        }
    };

    let lines = content.lines().count();

    files.push(FileContent {
        path: path.to_owned(),
        content,
        lines,
    });

    Ok(())
}

/// Build the full prompt with file contents formatted as markdown code blocks.
pub fn build_prompt(files: &[FileContent], question: &str, working_dir: &Path) -> String {
    let total_lines: usize = files.iter().map(|f| f.lines).sum();
    let mut prompt =
        String::with_capacity(files.iter().map(|f| f.content.len()).sum::<usize>() + 1000);

    prompt.push_str(&format!("Answer this question: {}\n\n", question));
    prompt.push_str(&format!(
        "**Files** ({} files, {} total lines):\n\n",
        files.len(),
        total_lines
    ));

    for file in files {
        let display_path = file.path.strip_prefix(working_dir).unwrap_or(&file.path);
        let ext = file.path.extension().and_then(|e| e.to_str()).unwrap_or("");

        prompt.push_str(&format!(
            "### {} ({} lines)\n```{}\n{}\n```\n\n",
            display_path.display(),
            file.lines,
            ext,
            file.content
        ));
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = tempfile::tempdir().unwrap();

        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"Hello\");\n}\n",
        )
        .unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub struct Foo;\n").unwrap();

        fs::write(dir.path().join(".gitignore"), "node_modules/\n*.log\n").unwrap();
        fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        fs::write(
            dir.path().join("node_modules/pkg.js"),
            "module.exports = {}",
        )
        .unwrap();
        fs::write(dir.path().join("debug.log"), "some logs").unwrap();

        dir
    }

    #[test]
    fn test_collect_files_basic() {
        let dir = setup_test_dir();
        let files = collect_files(&["src".to_string()], dir.path(), &None).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.path.ends_with("main.rs")));
        assert!(files.iter().any(|f| f.path.ends_with("lib.rs")));
    }

    #[test]
    fn test_collect_files_respects_gitignore() {
        let dir = setup_test_dir();
        let files = collect_files(&[".".to_string()], dir.path(), &None).unwrap();
        assert!(!files
            .iter()
            .any(|f| f.path.to_string_lossy().contains("node_modules")));
        assert!(!files
            .iter()
            .any(|f| f.path.to_string_lossy().contains(".log")));
    }

    #[test]
    fn test_collect_files_extension_filter() {
        let dir = setup_test_dir();
        fs::write(dir.path().join("src/script.py"), "print('hello')").unwrap();
        let files = collect_files(
            &["src".to_string()],
            dir.path(),
            &Some(vec!["py".to_string()]),
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].path.ends_with("script.py"));
    }

    #[test]
    fn test_collect_files_nonexistent_path() {
        let dir = setup_test_dir();
        let result = collect_files(&["nonexistent".to_string()], dir.path(), &None);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_prompt() {
        let files = vec![FileContent {
            path: PathBuf::from("/project/src/main.rs"),
            content: "fn main() {}".to_string(),
            lines: 1,
        }];
        let prompt = build_prompt(&files, "How does main work?", Path::new("/project"));
        assert!(prompt.contains("How does main work?"));
        assert!(prompt.contains("src/main.rs"));
        assert!(prompt.contains("fn main() {}"));
        assert!(prompt.contains("```rs"));
    }
}
