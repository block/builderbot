//! Review CRUD operations.

use rusqlite::{params, Connection, OptionalExtension};

use crate::git::Span;

use super::models::{Comment, CommentType, Review, ReviewScope};
use super::{now_timestamp, Store, StoreError};

impl Store {
    /// Create a new review.
    pub fn create_review(&self, review: &Review) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO reviews (id, branch_id, commit_sha, scope, session_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                review.id,
                review.branch_id,
                review.commit_sha,
                review.scope.as_str(),
                review.session_id,
                review.created_at,
                review.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Get or create a review for a (branch, commit, scope) triple.
    ///
    /// If a review already exists with that unique key, returns it (with
    /// all child data loaded). Otherwise creates a new empty review and
    /// returns it.
    ///
    /// When multiple reviews share the same triple, returns the most
    /// recently created one.
    pub fn ensure_review(
        &self,
        branch_id: &str,
        commit_sha: &str,
        scope: ReviewScope,
    ) -> Result<Review, StoreError> {
        let conn = self.conn.lock().unwrap();

        // Try to find existing — ORDER BY created_at DESC so we get the latest
        // when multiple reviews share the same (branch, commit, scope) triple.
        let existing: Option<Review> = conn
            .query_row(
                "SELECT id, branch_id, commit_sha, scope, session_id, created_at, updated_at
                 FROM reviews
                 WHERE branch_id = ?1 AND commit_sha = ?2 AND scope = ?3
                 ORDER BY created_at DESC LIMIT 1",
                params![branch_id, commit_sha, scope.as_str()],
                Self::row_to_review_header,
            )
            .optional()?;

        if let Some(mut review) = existing {
            Self::load_review_children(&conn, &mut review)?;
            return Ok(review);
        }

        // Create new
        let review = Review::new(branch_id, commit_sha, scope);
        conn.execute(
            "INSERT INTO reviews (id, branch_id, commit_sha, scope, session_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                review.id,
                review.branch_id,
                review.commit_sha,
                review.scope.as_str(),
                review.session_id,
                review.created_at,
                review.updated_at,
            ],
        )?;
        Ok(review)
    }

    /// Find a review by (branch, commit, scope) without creating one.
    ///
    /// When multiple reviews share the same triple, returns the most
    /// recently created one.
    pub fn find_review(
        &self,
        branch_id: &str,
        commit_sha: &str,
        scope: ReviewScope,
    ) -> Result<Option<Review>, StoreError> {
        let conn = self.conn.lock().unwrap();
        // ORDER BY created_at DESC so we get the latest when multiple
        // reviews share the same (branch, commit, scope) triple.
        let existing: Option<Review> = conn
            .query_row(
                "SELECT id, branch_id, commit_sha, scope, session_id, created_at, updated_at
                 FROM reviews
                 WHERE branch_id = ?1 AND commit_sha = ?2 AND scope = ?3
                 ORDER BY created_at DESC LIMIT 1",
                params![branch_id, commit_sha, scope.as_str()],
                Self::row_to_review_header,
            )
            .optional()?;

        match existing {
            Some(mut review) => {
                Self::load_review_children(&conn, &mut review)?;
                Ok(Some(review))
            }
            None => Ok(None),
        }
    }

    /// Get a review by id, loading all child data.
    pub fn get_review(&self, id: &str) -> Result<Option<Review>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let review = conn
            .query_row(
                "SELECT id, branch_id, commit_sha, scope, session_id, created_at, updated_at
                 FROM reviews WHERE id = ?1",
                params![id],
                Self::row_to_review_header,
            )
            .optional()?;

        match review {
            Some(mut r) => {
                Self::load_review_children(&conn, &mut r)?;
                Ok(Some(r))
            }
            None => Ok(None),
        }
    }

    /// List all reviews for a branch, with all child data loaded.
    pub fn list_reviews_for_branch(&self, branch_id: &str) -> Result<Vec<Review>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, branch_id, commit_sha, scope, session_id, created_at, updated_at
             FROM reviews WHERE branch_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_review_header)?;
        let mut reviews: Vec<Review> = rows.collect::<Result<Vec<_>, _>>()?;
        for review in &mut reviews {
            Self::load_review_children(&conn, review)?;
        }
        Ok(reviews)
    }

    /// Mark a file as reviewed.
    pub fn mark_reviewed(&self, review_id: &str, path: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO reviewed_files (review_id, path) VALUES (?1, ?2)",
            params![review_id, path],
        )?;
        Self::touch_review(&conn, review_id)?;
        Ok(())
    }

    /// Unmark a file as reviewed.
    pub fn unmark_reviewed(&self, review_id: &str, path: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM reviewed_files WHERE review_id = ?1 AND path = ?2",
            params![review_id, path],
        )?;
        Self::touch_review(&conn, review_id)?;
        Ok(())
    }

    /// Add a comment to a review.
    pub fn add_comment(&self, review_id: &str, comment: &Comment) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO comments (id, review_id, path, span_start, span_end, content, author, comment_type, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                comment.id,
                review_id,
                comment.path,
                comment.span.start,
                comment.span.end,
                comment.content,
                comment.author.as_str(),
                comment.comment_type.as_ref().map(|ct| ct.as_str()),
                comment.created_at,
            ],
        )?;
        Self::touch_review(&conn, review_id)?;
        Ok(())
    }

    /// Update a comment's content.
    pub fn update_comment(&self, comment_id: &str, content: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE comments SET content = ?1 WHERE id = ?2",
            params![content, comment_id],
        )?;
        // Touch the parent review
        let review_id: Option<String> = conn
            .query_row(
                "SELECT review_id FROM comments WHERE id = ?1",
                params![comment_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(rid) = review_id {
            Self::touch_review(&conn, &rid)?;
        }
        Ok(())
    }

    /// Delete a comment.
    pub fn delete_comment(&self, comment_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        // Get parent review before deleting
        let review_id: Option<String> = conn
            .query_row(
                "SELECT review_id FROM comments WHERE id = ?1",
                params![comment_id],
                |row| row.get(0),
            )
            .optional()?;
        conn.execute("DELETE FROM comments WHERE id = ?1", params![comment_id])?;
        if let Some(rid) = review_id {
            Self::touch_review(&conn, &rid)?;
        }
        Ok(())
    }

    /// Add a reference file path.
    pub fn add_reference_file(&self, review_id: &str, path: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO reference_files (review_id, path) VALUES (?1, ?2)",
            params![review_id, path],
        )?;
        Self::touch_review(&conn, review_id)?;
        Ok(())
    }

    /// Remove a reference file path.
    pub fn remove_reference_file(&self, review_id: &str, path: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM reference_files WHERE review_id = ?1 AND path = ?2",
            params![review_id, path],
        )?;
        Self::touch_review(&conn, review_id)?;
        Ok(())
    }

    /// Find a review by its session ID (for post-completion hooks).
    pub fn get_review_by_session(&self, session_id: &str) -> Result<Option<Review>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let review = conn
            .query_row(
                "SELECT id, branch_id, commit_sha, scope, session_id, created_at, updated_at
                 FROM reviews WHERE session_id = ?1",
                params![session_id],
                Self::row_to_review_header,
            )
            .optional()?;

        match review {
            Some(mut r) => {
                Self::load_review_children(&conn, &mut r)?;
                Ok(Some(r))
            }
            None => Ok(None),
        }
    }

    /// Delete an entire review and all associated data (cascades).
    pub fn delete_review(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM reviews WHERE id = ?1", params![id])?;
        Ok(())
    }

    // =========================================================================
    // Internal helpers
    // =========================================================================

    fn touch_review(conn: &Connection, review_id: &str) -> Result<(), StoreError> {
        conn.execute(
            "UPDATE reviews SET updated_at = ?1 WHERE id = ?2",
            params![now_timestamp(), review_id],
        )?;
        Ok(())
    }

    fn row_to_review_header(row: &rusqlite::Row) -> rusqlite::Result<Review> {
        let scope_str: String = row.get(3)?;
        Ok(Review {
            id: row.get(0)?,
            branch_id: row.get(1)?,
            commit_sha: row.get(2)?,
            scope: ReviewScope::parse(&scope_str).unwrap_or(ReviewScope::Commit),
            session_id: row.get(4)?,
            reviewed: Vec::new(),
            comments: Vec::new(),
            reference_files: Vec::new(),
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }

    fn load_review_children(conn: &Connection, review: &mut Review) -> Result<(), StoreError> {
        // Load reviewed files
        let mut stmt = conn.prepare("SELECT path FROM reviewed_files WHERE review_id = ?1")?;
        review.reviewed = stmt
            .query_map(params![&review.id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        // Load comments
        let mut stmt = conn.prepare(
            "SELECT id, path, span_start, span_end, content, author, comment_type, created_at
             FROM comments WHERE review_id = ?1 ORDER BY created_at ASC",
        )?;
        review.comments = stmt
            .query_map(params![&review.id], |row| {
                let author_str: String = row.get(5)?;
                let comment_type_str: Option<String> = row.get(6)?;
                Ok(Comment {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    span: Span::new(row.get(2)?, row.get(3)?),
                    content: row.get(4)?,
                    author: super::models::CommentAuthor::parse(&author_str)
                        .unwrap_or(super::models::CommentAuthor::User),
                    comment_type: comment_type_str.as_deref().and_then(CommentType::parse),
                    created_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Load reference files
        let mut stmt = conn.prepare("SELECT path FROM reference_files WHERE review_id = ?1")?;
        review.reference_files = stmt
            .query_map(params![&review.id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(())
    }
}
