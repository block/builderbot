//! Image CRUD operations and filesystem helpers.

use std::path::PathBuf;

use rusqlite::{params, OptionalExtension};

use super::models::Image;
use super::{Store, StoreError};

impl Store {
    pub fn create_image(&self, image: &Image) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO images (id, branch_id, project_id, session_id, filename, mime_type, size_bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                image.id,
                image.branch_id,
                image.project_id,
                image.session_id,
                image.filename,
                image.mime_type,
                image.size_bytes,
                image.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_image(&self, id: &str) -> Result<Option<Image>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, project_id, session_id, filename, mime_type, size_bytes, created_at
             FROM images WHERE id = ?1",
            params![id],
            Self::row_to_image,
        )
        .optional()
        .map_err(Into::into)
    }

    /// List images attached directly to a branch (not scoped to a session).
    ///
    /// Images with a `session_id` are excluded — those are session-scoped
    /// attachments that only appear in the session message history.
    pub fn list_images_for_branch(&self, branch_id: &str) -> Result<Vec<Image>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, branch_id, project_id, session_id, filename, mime_type, size_bytes, created_at
             FROM images WHERE branch_id = ?1 AND session_id IS NULL ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_image)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Return a filename that is unique among images on the given branch (or project if no branch).
    /// If `filename` is already taken, appends ` 2`, ` 3`, … before the extension
    /// (e.g. `Screenshot.png` → `Screenshot 2.png`).
    pub fn unique_image_filename(
        &self,
        branch_id: Option<&str>,
        project_id: &str,
        filename: &str,
    ) -> Result<String, StoreError> {
        let conn = self.conn.lock().unwrap();
        let existing: std::collections::HashSet<String> = match branch_id {
            Some(bid) => {
                let mut stmt = conn.prepare("SELECT filename FROM images WHERE branch_id = ?1")?;
                let rows = stmt.query_map(params![bid], |row| row.get::<_, String>(0))?;
                rows.filter_map(|r| r.ok()).collect()
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT filename FROM images WHERE project_id = ?1 AND branch_id IS NULL",
                )?;
                let rows = stmt.query_map(params![project_id], |row| row.get::<_, String>(0))?;
                rows.filter_map(|r| r.ok()).collect()
            }
        };

        if !existing.contains(filename) {
            return Ok(filename.to_string());
        }

        let path = std::path::Path::new(filename);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename);
        let ext = path.extension().and_then(|e| e.to_str());

        let mut counter = 2u32;
        loop {
            let candidate = match ext {
                Some(e) => format!("{stem} {counter}.{e}"),
                None => format!("{stem} {counter}"),
            };
            if !existing.contains(&candidate) {
                return Ok(candidate);
            }
            counter += 1;
        }
    }

    /// Associate images with a session so they are scoped to that session
    /// and excluded from the branch timeline.
    pub fn set_images_session_id(
        &self,
        image_ids: &[String],
        session_id: &str,
    ) -> Result<(), StoreError> {
        if image_ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        for id in image_ids {
            conn.execute(
                "UPDATE images SET session_id = ?1 WHERE id = ?2",
                params![session_id, id],
            )?;
        }
        Ok(())
    }

    pub fn delete_image(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM images WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Delete all images still marked as pending and remove their files from
    /// disk.  Called once at app startup to clean up images from compose
    /// sessions that were abandoned (e.g. the user quit the app mid-dialog).
    pub fn cleanup_pending_images(&self) -> Result<usize, StoreError> {
        use super::models::PENDING_SESSION_ID;

        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, project_id, filename FROM images WHERE session_id = ?1")?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map(params![PENDING_SESSION_ID], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let count = rows.len();
        for (id, project_id, filename) in &rows {
            if let Ok(path) = image_file_path(project_id, id, filename) {
                let _ = std::fs::remove_file(path);
            }
        }

        conn.execute(
            "DELETE FROM images WHERE session_id = ?1",
            params![PENDING_SESSION_ID],
        )?;

        Ok(count)
    }

    fn row_to_image(row: &rusqlite::Row) -> rusqlite::Result<Image> {
        Ok(Image {
            id: row.get(0)?,
            branch_id: row.get(1)?,
            project_id: row.get(2)?,
            session_id: row.get(3)?,
            filename: row.get(4)?,
            mime_type: row.get(5)?,
            size_bytes: row.get(6)?,
            created_at: row.get(7)?,
        })
    }
}

/// Compute the filesystem path for an image file.
/// Path: `<project_worktree_root>/images/<image_id>.<ext>`
pub fn image_file_path(
    project_id: &str,
    image_id: &str,
    filename: &str,
) -> Result<PathBuf, String> {
    let project_root = crate::git::project_worktree_root_for(project_id)
        .map_err(|e| format!("Cannot determine project root: {e}"))?;
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    Ok(project_root
        .join("images")
        .join(format!("{image_id}.{ext}")))
}
