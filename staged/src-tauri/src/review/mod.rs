//! Review storage using SQLite.
//!
//! Reviews are stored separately from git, keyed by DiffId.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::git::{DiffId, Span};

// =============================================================================
// Types
// =============================================================================

/// A review attached to a specific diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub id: DiffId,
    /// Paths that have been marked as reviewed
    pub reviewed: Vec<String>,
    /// Comments attached to specific locations
    pub comments: Vec<Comment>,
    /// Edits made during review (stored as diffs)
    pub edits: Vec<Edit>,
    /// Paths of reference files (files outside the diff that were viewed)
    pub reference_files: Vec<String>,
}

impl Review {
    pub fn new(id: DiffId) -> Self {
        Self {
            id,
            reviewed: Vec::new(),
            comments: Vec::new(),
            edits: Vec::new(),
            reference_files: Vec::new(),
        }
    }
}

/// Who authored a comment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CommentAuthor {
    User,
    Ai,
}

fn default_author() -> CommentAuthor {
    CommentAuthor::User
}

/// A comment attached to a specific location in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub path: String,
    pub span: Span,
    pub content: String,
    #[serde(default = "default_author")]
    pub author: CommentAuthor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

impl Comment {
    pub fn new(path: impl Into<String>, span: Span, content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            path: path.into(),
            span,
            content: content.into(),
            author: CommentAuthor::User,
            category: None,
            created_at: Some(chrono::Utc::now().to_rfc3339()),
        }
    }
}

/// An edit made during review, stored as a unified diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    pub id: String,
    pub path: String,
    /// Unified diff format
    pub diff: String,
}

impl Edit {
    pub fn new(path: impl Into<String>, diff: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            path: path.into(),
            diff: diff.into(),
        }
    }
}

/// Input for creating a new comment (from frontend).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewComment {
    pub path: String,
    pub span: Span,
    pub content: String,
}

/// Input for recording a new edit (from frontend).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEdit {
    pub path: String,
    pub diff: String,
}

// =============================================================================
// Error type
// =============================================================================

#[derive(Debug)]
pub struct ReviewError(pub String);

impl ReviewError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for ReviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ReviewError {}

impl From<rusqlite::Error> for ReviewError {
    fn from(e: rusqlite::Error) -> Self {
        ReviewError(e.to_string())
    }
}

type Result<T> = std::result::Result<T, ReviewError>;

// =============================================================================
// Global store
// =============================================================================

/// Global store instance - initialized during app setup.
static STORE: OnceLock<std::result::Result<ReviewStore, String>> = OnceLock::new();

/// Initialize the global store with the app's data directory.
/// Call this once during Tauri app setup.
pub fn init_store(app_handle: &AppHandle) -> Result<()> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| ReviewError::new(format!("Cannot get app data dir: {e}")))?;

    let db_path = app_data_dir.join("staged.db");

    // Migrate from old database name if needed
    let old_db_path = app_data_dir.join("reviews.db");
    if old_db_path.exists() && !db_path.exists() {
        if let Err(e) = std::fs::rename(&old_db_path, &db_path) {
            log::warn!("Failed to migrate reviews.db to staged.db: {e}");
        }
    }

    STORE.get_or_init(|| ReviewStore::open(db_path).map_err(|e| e.0));

    // Check if initialization succeeded
    get_store()?;
    Ok(())
}

/// Get the global store. Must call init_store first during app setup.
pub fn get_store() -> Result<&'static ReviewStore> {
    let result = STORE
        .get()
        .ok_or_else(|| ReviewError::new("Review store not initialized"))?;

    match result {
        Ok(store) => Ok(store),
        Err(msg) => Err(ReviewError::new(msg.clone())),
    }
}

// =============================================================================
// Review storage
// =============================================================================

/// Review storage backed by SQLite.
pub struct ReviewStore {
    conn: Mutex<Connection>,
}

impl ReviewStore {
    /// Open or create the review database at the given path.
    pub fn open(db_path: PathBuf) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ReviewError(format!("Cannot create directory: {e}")))?;
        }

        let conn = Connection::open(&db_path)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Initialize the database schema.
    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS reviews (
                before_ref TEXT NOT NULL,
                after_ref TEXT NOT NULL,
                PRIMARY KEY (before_ref, after_ref)
            );

            CREATE TABLE IF NOT EXISTS reviewed_files (
                before_ref TEXT NOT NULL,
                after_ref TEXT NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (before_ref, after_ref, path),
                FOREIGN KEY (before_ref, after_ref) REFERENCES reviews(before_ref, after_ref) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS comments (
                id TEXT PRIMARY KEY,
                before_ref TEXT NOT NULL,
                after_ref TEXT NOT NULL,
                path TEXT NOT NULL,
                span_start INTEGER NOT NULL,
                span_end INTEGER NOT NULL,
                content TEXT NOT NULL,
                FOREIGN KEY (before_ref, after_ref) REFERENCES reviews(before_ref, after_ref) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS edits (
                id TEXT PRIMARY KEY,
                before_ref TEXT NOT NULL,
                after_ref TEXT NOT NULL,
                path TEXT NOT NULL,
                diff TEXT NOT NULL,
                FOREIGN KEY (before_ref, after_ref) REFERENCES reviews(before_ref, after_ref) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS reference_files (
                before_ref TEXT NOT NULL,
                after_ref TEXT NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (before_ref, after_ref, path),
                FOREIGN KEY (before_ref, after_ref) REFERENCES reviews(before_ref, after_ref) ON DELETE CASCADE
            );

            -- AI analysis results
            CREATE TABLE IF NOT EXISTS ai_changeset_summary (
                before_ref TEXT NOT NULL,
                after_ref TEXT NOT NULL,
                summary_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (before_ref, after_ref),
                FOREIGN KEY (before_ref, after_ref) REFERENCES reviews(before_ref, after_ref) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS ai_file_analysis (
                before_ref TEXT NOT NULL,
                after_ref TEXT NOT NULL,
                path TEXT NOT NULL,
                result_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (before_ref, after_ref, path),
                FOREIGN KEY (before_ref, after_ref) REFERENCES reviews(before_ref, after_ref) ON DELETE CASCADE
            );

            PRAGMA foreign_keys = ON;
            "#,
        )?;

        // Migration: Add new columns to comments table if they don't exist
        // Note: SQLite doesn't have "IF NOT EXISTS" for ALTER TABLE, so we check each column
        // individually to handle partial migration states gracefully.
        Self::migrate_add_column(&conn, "comments", "author", "TEXT NOT NULL DEFAULT 'user'")?;
        Self::migrate_add_column(&conn, "comments", "category", "TEXT")?;
        Self::migrate_add_column(&conn, "comments", "created_at", "TEXT")?;

        Ok(())
    }

    /// Helper method to add a column to a table if it doesn't already exist.
    fn migrate_add_column(
        conn: &Connection,
        table: &str,
        column: &str,
        column_type: &str,
    ) -> Result<()> {
        // Check if column exists by querying table info
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        if !columns.contains(&column.to_string()) {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {column_type}"),
                [],
            )?;
        }

        Ok(())
    }

    /// Get or create a review for the given diff.
    pub fn get_or_create(&self, id: &DiffId) -> Result<Review> {
        let conn = self.conn.lock().unwrap();

        // Ensure review exists
        conn.execute(
            "INSERT OR IGNORE INTO reviews (before_ref, after_ref) VALUES (?1, ?2)",
            params![&id.before, &id.after],
        )?;

        self.get_with_conn(&conn, id)
    }

    /// Get a review by its DiffId.
    pub fn get(&self, id: &DiffId) -> Result<Review> {
        let conn = self.conn.lock().unwrap();
        self.get_with_conn(&conn, id)
    }

    /// Get a review using an existing connection lock.
    fn get_with_conn(&self, conn: &Connection, id: &DiffId) -> Result<Review> {
        // Check if review exists
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM reviews WHERE before_ref = ?1 AND after_ref = ?2",
                params![&id.before, &id.after],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        if !exists {
            return Ok(Review::new(id.clone()));
        }

        // Load reviewed files
        let mut stmt = conn
            .prepare("SELECT path FROM reviewed_files WHERE before_ref = ?1 AND after_ref = ?2")?;
        let reviewed: Vec<String> = stmt
            .query_map(params![&id.before, &id.after], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut stmt = conn.prepare(
            "SELECT id, path, span_start, span_end, content, author, category, created_at
             FROM comments WHERE before_ref = ?1 AND after_ref = ?2",
        )?;
        let comments: Vec<Comment> = stmt
            .query_map(params![&id.before, &id.after], |row| {
                let author_str: String = row.get(5).unwrap_or_else(|_| "user".to_string());
                let author = match author_str.as_str() {
                    "ai" => CommentAuthor::Ai,
                    _ => CommentAuthor::User,
                };

                Ok(Comment {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    span: Span::new(row.get(2)?, row.get(3)?),
                    content: row.get(4)?,
                    author,
                    category: row.get(6).ok(),
                    created_at: row.get(7).ok(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Load edits
        let mut stmt = conn
            .prepare("SELECT id, path, diff FROM edits WHERE before_ref = ?1 AND after_ref = ?2")?;
        let edits: Vec<Edit> = stmt
            .query_map(params![&id.before, &id.after], |row| {
                Ok(Edit {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    diff: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Load reference files
        let mut stmt = conn
            .prepare("SELECT path FROM reference_files WHERE before_ref = ?1 AND after_ref = ?2")?;
        let reference_files: Vec<String> = stmt
            .query_map(params![&id.before, &id.after], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(Review {
            id: id.clone(),
            reviewed,
            comments,
            edits,
            reference_files,
        })
    }

    /// Mark a file as reviewed.
    pub fn mark_reviewed(&self, id: &DiffId, path: &str) -> Result<()> {
        self.get_or_create(id)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO reviewed_files (before_ref, after_ref, path) VALUES (?1, ?2, ?3)",
            params![&id.before, &id.after, path],
        )?;
        Ok(())
    }

    /// Unmark a file as reviewed.
    pub fn unmark_reviewed(&self, id: &DiffId, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM reviewed_files WHERE before_ref = ?1 AND after_ref = ?2 AND path = ?3",
            params![&id.before, &id.after, path],
        )?;
        Ok(())
    }

    /// Add a comment.
    pub fn add_comment(&self, id: &DiffId, comment: &Comment) -> Result<()> {
        self.get_or_create(id)?;
        let conn = self.conn.lock().unwrap();

        let author_str = match comment.author {
            CommentAuthor::User => "user",
            CommentAuthor::Ai => "ai",
        };

        conn.execute(
            "INSERT INTO comments (id, before_ref, after_ref, path, span_start, span_end, content, author, category, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &comment.id,
                &id.before,
                &id.after,
                &comment.path,
                comment.span.start,
                comment.span.end,
                &comment.content,
                author_str,
                &comment.category,
                &comment.created_at
            ],
        )?;
        Ok(())
    }

    /// Update a comment's content.
    pub fn update_comment(&self, comment_id: &str, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE comments SET content = ?1 WHERE id = ?2",
            params![content, comment_id],
        )?;
        Ok(())
    }

    /// Delete a comment.
    pub fn delete_comment(&self, comment_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM comments WHERE id = ?1", params![comment_id])?;
        Ok(())
    }

    /// Add an edit.
    pub fn add_edit(&self, id: &DiffId, edit: &Edit) -> Result<()> {
        self.get_or_create(id)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO edits (id, before_ref, after_ref, path, diff) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&edit.id, &id.before, &id.after, &edit.path, &edit.diff],
        )?;
        Ok(())
    }

    /// Delete an edit.
    pub fn delete_edit(&self, edit_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM edits WHERE id = ?1", params![edit_id])?;
        Ok(())
    }

    /// Add a reference file path.
    pub fn add_reference_file(&self, id: &DiffId, path: &str) -> Result<()> {
        self.get_or_create(id)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO reference_files (before_ref, after_ref, path) VALUES (?1, ?2, ?3)",
            params![&id.before, &id.after, path],
        )?;
        Ok(())
    }

    /// Remove a reference file path.
    pub fn remove_reference_file(&self, id: &DiffId, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM reference_files WHERE before_ref = ?1 AND after_ref = ?2 AND path = ?3",
            params![&id.before, &id.after, path],
        )?;
        Ok(())
    }

    /// Delete an entire review and all associated data.
    pub fn delete(&self, id: &DiffId) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Foreign key cascades handle child tables
        conn.execute(
            "DELETE FROM reviews WHERE before_ref = ?1 AND after_ref = ?2",
            params![&id.before, &id.after],
        )?;
        Ok(())
    }
}

// =============================================================================
// Export
// =============================================================================

/// Export a review as markdown for clipboard.
pub fn export_markdown(review: &Review) -> String {
    let mut md = String::new();

    // Group comments by file
    let mut comments_by_file: std::collections::HashMap<&str, Vec<&Comment>> =
        std::collections::HashMap::new();
    for comment in &review.comments {
        comments_by_file
            .entry(&comment.path)
            .or_default()
            .push(comment);
    }

    // Group edits by file
    let mut edits_by_file: std::collections::HashMap<&str, Vec<&Edit>> =
        std::collections::HashMap::new();
    for edit in &review.edits {
        edits_by_file.entry(&edit.path).or_default().push(edit);
    }

    // Collect all files
    let mut all_files: Vec<&str> = comments_by_file
        .keys()
        .chain(edits_by_file.keys())
        .copied()
        .collect();
    all_files.sort();
    all_files.dedup();

    for file in all_files {
        md.push_str(&format!("## {file}\n\n"));

        if let Some(comments) = comments_by_file.get(file) {
            for comment in comments {
                let span = &comment.span;
                let location = if span.end == span.start + 1 {
                    format!("Line {}", span.start + 1)
                } else {
                    format!("Lines {}-{}", span.start + 1, span.end)
                };
                md.push_str(&format!("- **{}**: {}\n", location, comment.content));
            }
            md.push('\n');
        }

        if let Some(edits) = edits_by_file.get(file) {
            for edit in edits {
                md.push_str("**Edit applied:**\n```diff\n");
                md.push_str(&edit.diff);
                if !edit.diff.ends_with('\n') {
                    md.push('\n');
                }
                md.push_str("```\n\n");
            }
        }
    }

    if md.is_empty() {
        md.push_str("No comments or edits.\n");
    }

    md
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_mark_reviewed() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = ReviewStore::open(db_path).unwrap();
        let id = DiffId::new("main", "feature");

        store.mark_reviewed(&id, "src/main.rs").unwrap();
        let review = store.get(&id).unwrap();
        assert_eq!(review.reviewed, vec!["src/main.rs"]);

        store.unmark_reviewed(&id, "src/main.rs").unwrap();
        let review = store.get(&id).unwrap();
        assert!(review.reviewed.is_empty());
    }

    #[test]
    fn test_comments() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = ReviewStore::open(db_path).unwrap();
        let id = DiffId::new("main", "feature");

        // Single line comment (line 42 = span 42..43)
        let comment = Comment::new("src/lib.rs", Span::new(42, 43), "This looks wrong");

        store.add_comment(&id, &comment).unwrap();
        let review = store.get(&id).unwrap();
        assert_eq!(review.comments.len(), 1);
        assert_eq!(review.comments[0].content, "This looks wrong");
        assert_eq!(review.comments[0].span.start, 42);
        assert_eq!(review.comments[0].span.end, 43);

        store
            .update_comment(&comment.id, "Actually it's fine")
            .unwrap();
        let review = store.get(&id).unwrap();
        assert_eq!(review.comments[0].content, "Actually it's fine");

        store.delete_comment(&comment.id).unwrap();
        let review = store.get(&id).unwrap();
        assert!(review.comments.is_empty());
    }

    #[test]
    fn test_edits() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = ReviewStore::open(db_path).unwrap();
        let id = DiffId::new("main", "feature");

        let edit = Edit::new("src/lib.rs", "-old\n+new");

        store.add_edit(&id, &edit).unwrap();
        let review = store.get(&id).unwrap();
        assert_eq!(review.edits.len(), 1);
        assert_eq!(review.edits[0].diff, "-old\n+new");

        store.delete_edit(&edit.id).unwrap();
        let review = store.get(&id).unwrap();
        assert!(review.edits.is_empty());
    }

    #[test]
    fn test_delete_review() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = ReviewStore::open(db_path).unwrap();
        let id = DiffId::new("main", "feature");

        store.mark_reviewed(&id, "src/main.rs").unwrap();
        // Range comment spanning lines 0-10
        store
            .add_comment(&id, &Comment::new("src/main.rs", Span::new(0, 10), "test"))
            .unwrap();

        store.delete(&id).unwrap();
        let review = store.get(&id).unwrap();
        assert!(review.reviewed.is_empty());
        assert!(review.comments.is_empty());
    }

    #[test]
    fn test_export_markdown() {
        let id = DiffId::new("main", "feature");
        let mut review = Review::new(id);

        // Single line comment on line 10 (0-indexed)
        review.comments.push(Comment {
            id: "c1".into(),
            path: "src/lib.rs".into(),
            span: Span::new(10, 11),
            content: "Fix this".into(),
            author: CommentAuthor::User,
            category: None,
            created_at: None,
        });

        review.edits.push(Edit {
            id: "e1".into(),
            path: "src/lib.rs".into(),
            diff: "-old\n+new".into(),
        });

        let md = export_markdown(&review);
        assert!(md.contains("## src/lib.rs"));
        assert!(md.contains("Line 11")); // 0-indexed to 1-indexed
        assert!(md.contains("Fix this"));
        assert!(md.contains("-old"));
    }
}
