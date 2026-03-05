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

    pub fn list_images_for_branch(&self, branch_id: &str) -> Result<Vec<Image>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, branch_id, project_id, session_id, filename, mime_type, size_bytes, created_at
             FROM images WHERE branch_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_image)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn delete_image(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM images WHERE id = ?1", params![id])?;
        Ok(())
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
