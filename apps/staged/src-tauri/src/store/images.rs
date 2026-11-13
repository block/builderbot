//! Image CRUD operations and filesystem helpers.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, OptionalExtension};

use super::models::{Image, ImageDimensions};
use super::{Store, StoreChange, StoreError};

const IMAGE_COLUMNS: &str = "id, branch_id, project_id, session_id, filename, mime_type, size_bytes, created_at, width, height, density";

/// Stored in the dimension columns of a raster image whose bytes were read
/// (probed on attach, or read back from disk by the backfill) and yielded no
/// usable header: distinct from NULL (never read) so the backfill does not
/// open it again, and read back as `dimensions: None` since a real density
/// is 1..=4.
const UNREADABLE_DIMENSIONS: ImageDimensions = ImageDimensions {
    width: 0,
    height: 0,
    density: 0,
};

/// The note bodies that embed any stored media, leaving out note `?1`. Note
/// ids are UUIDs, so one id names at most one row across both tables.
const NOTE_BODIES_SQL: [&str; 2] = [
    "SELECT content FROM notes
      WHERE id IS NOT ?1 AND instr(content, 'staged-media://') > 0",
    "SELECT content FROM project_notes
      WHERE id IS NOT ?1 AND instr(content, 'staged-media://') > 0",
];

/// The attachment list of every sent and queued chat message that has one.
///
/// `image_ids IS NOT NULL` is the predicate of `idx_session_messages_image_ids`
/// (migration 0049, a partial index over the messages that attach something),
/// so the first statement reads that index and never a row of
/// `session_messages`. `images_tests.rs` pins the plan.
pub(super) const CHAT_ATTACHMENTS_SQL: [&str; 2] = [
    "SELECT image_ids FROM session_messages WHERE image_ids IS NOT NULL",
    "SELECT image_ids FROM queued_session_messages WHERE image_ids IS NOT NULL",
];

/// Whether any sent or queued chat message attaches the image whose quoted id
/// is `?1`: the per-image form of [`CHAT_ATTACHMENTS_SQL`], kept for the tests
/// that hold the batched pass to it.
///
/// `image_ids IS NOT NULL` changes no result, since `instr` of NULL is NULL,
/// but it is what lets SQLite read `idx_session_messages_image_ids` instead of
/// every row of `session_messages`: the planner does not infer the index's
/// `WHERE` from `instr(...) > 0`.
#[cfg(test)]
pub(super) const CHAT_REFERENCE_SQL: &str = "SELECT EXISTS(
       SELECT 1 FROM session_messages
        WHERE image_ids IS NOT NULL AND instr(image_ids, ?1) > 0
       UNION ALL
       SELECT 1 FROM queued_session_messages
        WHERE image_ids IS NOT NULL AND instr(image_ids, ?1) > 0)";

/// The images and PDFs linked to session `?1`, before the ones a note embeds
/// are dropped ([`Store::get_image_ids_for_session`]). Videos and other note
/// media are linked to a session too and are never chat attachments.
///
/// Nothing indexes `images.session_id`, so this reads the `images` table: one
/// row per stored file, small beside the note bodies it used to be joined to.
const SESSION_ATTACHMENTS_SQL: &str = "SELECT id FROM images
      WHERE session_id = ?1
        AND (mime_type LIKE 'image/%' OR mime_type = 'application/pdf')";

/// [`Store::get_image_ids_for_session`] as one statement, with the note
/// check as two correlated scans of the note tables for each attachment:
/// the query it ran before the note pass, kept for the test that holds the
/// two to the same answer.
#[cfg(test)]
const SESSION_ATTACHMENTS_PER_IMAGE_SQL: &str = "SELECT id FROM images WHERE session_id = ?1
             AND (mime_type LIKE 'image/%' OR mime_type = 'application/pdf')
             AND NOT EXISTS (SELECT 1 FROM notes WHERE instr(content, 'staged-media://' || images.id || '.') > 0)
             AND NOT EXISTS (SELECT 1 FROM project_notes WHERE instr(content, 'staged-media://' || images.id || '.') > 0)";

/// What still uses each of a set of candidate media ids, read in one pass
/// over the note bodies and the chat attachment lists instead of one pass per
/// id ([`Store::media_references`]).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MediaReferences {
    /// The ids the pass was asked about: the only ones it can clear.
    asked: HashSet<String>,
    notes: HashSet<String>,
    chat: HashSet<String>,
}

impl MediaReferences {
    /// Whether a note embeds `id`, not counting the note the pass left out.
    pub(crate) fn in_notes(&self, id: &str) -> bool {
        self.notes.contains(id)
    }

    /// Whether a sent or queued chat message attaches `id`.
    pub(crate) fn in_chat(&self, id: &str) -> bool {
        self.chat.contains(id)
    }

    /// Whether nothing the pass read uses `id`. With a note left out, that
    /// note is the only thing using it, so the row can follow the note to
    /// another scope without breaking anything else.
    ///
    /// This is the answer that lets a caller delete a file or take a row
    /// along, so it is given only for an id the pass was asked about. The
    /// pass looked for nothing else, and finding no use of an id it never
    /// looked for says nothing: any other id is reported as in use.
    pub(crate) fn unused(&self, id: &str) -> bool {
        self.asked.contains(id) && !self.in_notes(id) && !self.in_chat(id)
    }
}

/// How many passes over the note bodies and over the chat attachment lists
/// this thread has made, so a test can hold an operation to one of each.
#[cfg(test)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReferencePasses {
    pub notes: usize,
    pub chat: usize,
}

#[cfg(test)]
thread_local! {
    static REFERENCE_PASSES: std::cell::Cell<ReferencePasses> =
        const { std::cell::Cell::new(ReferencePasses { notes: 0, chat: 0 }) };
}

/// The passes made on this thread since the last call, resetting the count.
#[cfg(test)]
pub(crate) fn take_reference_passes() -> ReferencePasses {
    REFERENCE_PASSES.with(std::cell::Cell::take)
}

#[cfg(test)]
fn count_reference_pass(count: impl FnOnce(&mut ReferencePasses)) {
    REFERENCE_PASSES.with(|passes| {
        let mut current = passes.get();
        count(&mut current);
        passes.set(current);
    });
}

/// The ids whose needle occurs in a text `statements` yield, each run with
/// `params` and read in turn until every needle is found. `needles` pairs a
/// needle with its id.
///
/// The needles are matched as plain substrings, as `instr` matched them one
/// id at a time, on the bytes SQLite holds: a value that is not valid UTF-8
/// is still searched rather than failing the pass.
fn ids_found(
    conn: &rusqlite::Connection,
    statements: &[&str],
    params: &[&dyn rusqlite::ToSql],
    mut needles: Vec<(String, &str)>,
) -> Result<HashSet<String>, StoreError> {
    let mut found = HashSet::new();
    for sql in statements {
        if needles.is_empty() {
            break;
        }
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        while let Some(row) = rows.next()? {
            let value = row.get_ref(0)?;
            let text =
                String::from_utf8_lossy(value.as_bytes().map_err(|e| StoreError(e.to_string()))?);
            needles.retain(|(needle, id)| {
                let hit = text.contains(needle.as_str());
                if hit {
                    found.insert((*id).to_owned());
                }
                !hit
            });
            if needles.is_empty() {
                break;
            }
        }
    }
    Ok(found)
}

impl Store {
    /// Which of `ids` are still in use, leaving `except_note_id` (a branch or
    /// project note) out of the count: one pass over the note bodies and one
    /// over the chat attachment lists, under one lock, however many ids are
    /// asked about.
    ///
    /// Conservative reference check: even a reference inside an example keeps
    /// its file. No note ownership columns or reconciliation state are needed.
    pub(crate) fn media_references<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
        except_note_id: Option<&str>,
    ) -> Result<MediaReferences, StoreError> {
        let ids: Vec<&str> = ids.into_iter().collect();
        if ids.is_empty() {
            return Ok(MediaReferences::default());
        }
        let conn = self.conn.lock().unwrap();
        Self::media_references_on(&conn, ids, except_note_id)
    }

    /// [`Self::media_references`] on an open connection, so a transaction can
    /// confirm what it is about to re-home under its own snapshot.
    pub(super) fn media_references_on<'a>(
        conn: &rusqlite::Connection,
        ids: impl IntoIterator<Item = &'a str>,
        except_note_id: Option<&str>,
    ) -> Result<MediaReferences, StoreError> {
        let ids: Vec<&str> = ids.into_iter().collect();
        Ok(MediaReferences {
            asked: ids.iter().map(|id| (*id).to_owned()).collect(),
            notes: Self::embedded_in_notes(conn, &ids, except_note_id)?,
            chat: Self::attached_in_chat(conn, &ids)?,
        })
    }

    /// Which of `ids` some note embeds: the note half of
    /// [`Self::media_references`], for a caller with no use for the chat half.
    pub(crate) fn media_embedded_in_notes<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<HashSet<String>, StoreError> {
        let ids: Vec<&str> = ids.into_iter().collect();
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        let conn = self.conn.lock().unwrap();
        Self::embedded_in_notes(&conn, &ids, None)
    }

    /// Which of `ids` a note other than `except_note_id` embeds.
    fn embedded_in_notes(
        conn: &rusqlite::Connection,
        ids: &[&str],
        except_note_id: Option<&str>,
    ) -> Result<HashSet<String>, StoreError> {
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        #[cfg(test)]
        count_reference_pass(|passes| passes.notes += 1);
        let needles = ids
            .iter()
            .map(|id| (format!("staged-media://{id}."), *id))
            .collect();
        ids_found(conn, &NOTE_BODIES_SQL, params![except_note_id], needles)
    }

    /// Which of `ids` a sent or queued chat message attaches.
    fn attached_in_chat(
        conn: &rusqlite::Connection,
        ids: &[&str],
    ) -> Result<HashSet<String>, StoreError> {
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        #[cfg(test)]
        count_reference_pass(|passes| passes.chat += 1);
        // Image IDs are UUIDs; quote the exact JSON string to avoid substrings.
        let needles = ids.iter().map(|id| (format!("\"{id}\""), *id)).collect();
        ids_found(conn, &CHAT_ATTACHMENTS_SQL, params![], needles)
    }
}

/// The checks one id at a time, as every caller ran them before
/// [`Store::media_references`] answered for a whole set in one pass, and the
/// session attachment lookup as it ran before it used that pass. Nothing
/// outside the tests calls these now; they stay as the definition the batched
/// pass is held to (`images_tests.rs`).
#[cfg(test)]
impl Store {
    pub(crate) fn media_is_referenced(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::referenced_by_notes(&conn, id, None)
    }

    pub(crate) fn image_has_chat_references(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::referenced_by_chat(&conn, id)
    }

    /// Whether `note_id` (a branch or project note) is the only thing using
    /// media `id`: no other note embeds it and no chat message attaches it.
    pub(crate) fn media_is_exclusive_to(
        &self,
        id: &str,
        note_id: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        Ok(!Self::referenced_by_notes(&conn, id, Some(note_id))?
            && !Self::referenced_by_chat(&conn, id)?)
    }

    /// Whether any note other than `except_note_id` embeds media `id`.
    pub(super) fn referenced_by_notes(
        conn: &rusqlite::Connection,
        id: &str,
        except_note_id: Option<&str>,
    ) -> Result<bool, StoreError> {
        let needle = format!("staged-media://{id}.");
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM notes WHERE instr(content, ?1) > 0 AND id IS NOT ?2
             UNION ALL SELECT 1 FROM project_notes WHERE instr(content, ?1) > 0 AND id IS NOT ?2)",
            params![needle, except_note_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub(super) fn referenced_by_chat(
        conn: &rusqlite::Connection,
        id: &str,
    ) -> Result<bool, StoreError> {
        // Image IDs are UUIDs; quote the exact JSON string to avoid substrings.
        let needle = format!("\"{id}\"");
        conn.query_row(CHAT_REFERENCE_SQL, params![needle], |row| row.get(0))
            .map_err(Into::into)
    }

    /// [`Self::get_image_ids_for_session`] as it ran before the note pass.
    pub(crate) fn get_image_ids_for_session_per_image(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(SESSION_ATTACHMENTS_PER_IMAGE_SQL)?;
        let ids = stmt
            .query_map(params![session_id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }
}

impl Store {
    pub fn create_image(&self, image: &Image) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::insert_image(&conn, image)?;
        // Only branch-attached images without a session appear on the
        // timeline; session-scoped attachments live in the chat history,
        // which polls.
        if image.session_id.is_none() {
            if let Some(branch_id) = &image.branch_id {
                self.publish_with(|| StoreChange::Branch {
                    branch_id: branch_id.clone(),
                    project_id: Some(image.project_id.clone()),
                });
            }
        }
        Ok(())
    }

    pub(super) fn insert_image(
        conn: &rusqlite::Connection,
        image: &Image,
    ) -> Result<(), StoreError> {
        // Every raster row inserted since the columns exist has had its
        // bytes probed on the way in (`write_media` and the note-transfer
        // copy are the producers), so None on an `image/*` row means the
        // header could not be read: store the sentinel now rather than let
        // the backfill read the file once more to learn the same thing.
        // Videos and PDFs have nothing to read and stay NULL; only rows from
        // before migration 0033 are raster and NULL.
        let dimensions = image.dimensions.or_else(|| {
            image
                .mime_type
                .starts_with("image/")
                .then_some(UNREADABLE_DIMENSIONS)
        });
        conn.execute(
            &format!(
                "INSERT INTO images ({IMAGE_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ),
            params![
                image.id,
                image.branch_id,
                image.project_id,
                image.session_id,
                image.filename,
                image.mime_type,
                image.size_bytes,
                image.created_at,
                dimensions.map(|d| d.width),
                dimensions.map(|d| d.height),
                dimensions.map(|d| d.density),
            ],
        )?;
        Ok(())
    }

    /// Put a row back to how migration 0033 left it, for tests that need a
    /// raster row whose file has never been read; nothing in the app makes
    /// one.
    #[cfg(test)]
    pub(crate) fn forget_image_dimensions(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE images SET width = NULL, height = NULL, density = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Record what reading an image's file for its dimensions found, for rows
    /// that predate the columns (`get_media_dimensions` reads them from disk
    /// the first time a note embedding the image is opened). `None` stores
    /// [`UNREADABLE_DIMENSIONS`] so the file is consulted once, not on every
    /// open. Nothing on the timeline shows these, so no change is published.
    pub fn set_image_dimensions(
        &self,
        id: &str,
        dimensions: Option<ImageDimensions>,
    ) -> Result<(), StoreError> {
        let ImageDimensions {
            width,
            height,
            density,
        } = dimensions.unwrap_or(UNREADABLE_DIMENSIONS);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE images SET width = ?1, height = ?2, density = ?3 WHERE id = ?4",
            params![width, height, density, id],
        )?;
        Ok(())
    }

    /// Whether an image's file has been read for its dimensions: false only
    /// for rows that predate the columns and have not been read back yet. A
    /// file that was read and gave nothing holds the sentinel, which comes
    /// back as `dimensions: None` like NULL does, so this is what tells the
    /// backfill not to read it again.
    pub fn image_dimensions_recorded(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT density IS NOT NULL FROM images WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map(|recorded| recorded.unwrap_or(false))
        .map_err(Into::into)
    }

    pub fn get_image(&self, id: &str) -> Result<Option<Image>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {IMAGE_COLUMNS} FROM images WHERE id = ?1"),
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
        let mut stmt = conn.prepare(&format!(
            "SELECT {IMAGE_COLUMNS} FROM images
             WHERE branch_id = ?1 AND session_id IS NULL ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_image)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Every image row attached to a branch, session-scoped ones included.
    ///
    /// [`Self::list_images_for_branch`] is the timeline view and stops at
    /// branch-level attachments; this is the whole set, for callers that have to
    /// account for the files on disk (moving a branch between projects).
    pub fn list_all_images_for_branch(&self, branch_id: &str) -> Result<Vec<Image>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {IMAGE_COLUMNS} FROM images WHERE branch_id = ?1 ORDER BY created_at ASC"
        ))?;
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
        self.unique_image_filename_excluding(branch_id, project_id, filename, &HashSet::new())
    }

    /// [`Self::unique_image_filename`] that also steers clear of `reserved`:
    /// names a caller has already handed out but not inserted yet, so a batch
    /// of rows staged together stays unique among itself as well as against
    /// the table.
    pub fn unique_image_filename_excluding(
        &self,
        branch_id: Option<&str>,
        project_id: &str,
        filename: &str,
        reserved: &HashSet<String>,
    ) -> Result<String, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut existing: HashSet<String> = match branch_id {
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
        existing.extend(reserved.iter().cloned());

        if !existing.contains(filename) {
            return Ok(filename.to_string());
        }

        let (stem, ext) = split_filename(filename);
        let mut counter = 2u32;
        loop {
            let candidate = numbered_filename(stem, ext, counter);
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

    /// Return chat attachments linked to a session, excluding note-owned media
    /// so restarted sessions don't send videos as ACP image input. PDFs are
    /// chat attachments too; the runner renders their pages for the prompt.
    ///
    /// The ids come back in the order the table yields them. Whatever a note
    /// embeds is dropped by one pass over the note bodies for the whole set,
    /// the note half of [`Self::media_references`], under the same lock,
    /// rather than by a scan of both note tables for each attachment. A
    /// session with nothing attached makes no pass.
    pub fn get_image_ids_for_session(&self, session_id: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(SESSION_ATTACHMENTS_SQL)?;
        let mut ids = stmt
            .query_map(params![session_id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        let embedded = {
            let candidates: Vec<&str> = ids.iter().map(String::as_str).collect();
            Self::embedded_in_notes(&conn, &candidates, None)?
        };
        ids.retain(|id| !embedded.contains(id));
        Ok(ids)
    }

    pub fn delete_image(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        // Timeline-visible only when branch-attached with no session scope.
        let branch_id = Self::lookup_id(
            &conn,
            "SELECT branch_id FROM images WHERE id = ?1 AND session_id IS NULL",
            id,
        );
        conn.execute("DELETE FROM images WHERE id = ?1", params![id])?;
        if let Some(branch_id) = branch_id {
            self.publish_with(|| StoreChange::Branch {
                project_id: Self::branch_project_id(&conn, &branch_id),
                branch_id,
            });
        }
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
            remove_image_files(project_id, id, filename);
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
            dimensions: match (row.get(8)?, row.get(9)?, row.get(10)?) {
                (Some(width), Some(height), Some(density)) if density > 0 => {
                    Some(ImageDimensions {
                        width,
                        height,
                        density,
                    })
                }
                _ => None,
            },
        })
    }
}

fn split_filename(filename: &str) -> (&str, Option<&str>) {
    let path = std::path::Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    (stem, path.extension().and_then(|e| e.to_str()))
}

fn numbered_filename(stem: &str, ext: Option<&str>, counter: u32) -> String {
    match ext {
        Some(e) => format!("{stem} {counter}.{e}"),
        None => format!("{stem} {counter}"),
    }
}

/// Whether `stored` is `filename` itself or one of the ` 2`, ` 3`, … variants
/// [`Store::unique_image_filename`] produces for it. Callers that only know
/// the name a file was saved from use this to find its row.
pub(crate) fn is_unique_filename_variant(stored: &str, filename: &str) -> bool {
    if stored == filename {
        return true;
    }
    let (stem, ext) = split_filename(filename);
    let Some(rest) = stored
        .strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return false;
    };
    let counter = match ext {
        Some(e) => rest.strip_suffix(e).and_then(|rest| rest.strip_suffix('.')),
        None => Some(rest),
    };
    counter.is_some_and(|counter| {
        counter.bytes().all(|b| b.is_ascii_digit()) && counter.parse::<u32>().is_ok_and(|n| n >= 2)
    })
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

/// Where the rendered first-page thumbnail of a PDF attachment is cached,
/// beside the PDF itself: `<project_worktree_root>/images/<image_id>.thumb.png`.
/// The `.thumb.png` suffix cannot collide with [`image_file_path`], whose
/// names are a bare UUID plus one extension.
pub fn thumbnail_file_path(project_id: &str, image_id: &str) -> Result<PathBuf, String> {
    let project_root = crate::git::project_worktree_root_for(project_id)
        .map_err(|e| format!("Cannot determine project root: {e}"))?;
    Ok(project_root
        .join("images")
        .join(format!("{image_id}.thumb.png")))
}

/// Where a thumbnail is written before it is renamed over
/// [`thumbnail_file_path`], so a reader sees either no cache or a complete
/// one: `<project_worktree_root>/images/<image_id>.thumb.<pid>-<n>.tmp`.
/// Unique per writer, so two slots rendering the same new attachment at
/// once never interleave into one file; whichever rename lands last wins,
/// and both are complete PNGs of the same page.
pub fn thumbnail_temp_file_path(project_id: &str, image_id: &str) -> Result<PathBuf, String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let project_root = crate::git::project_worktree_root_for(project_id)
        .map_err(|e| format!("Cannot determine project root: {e}"))?;
    Ok(project_root.join("images").join(format!(
        "{image_id}.thumb.{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )))
}

/// Best-effort removal of an image's file, any cached thumbnail, and any
/// thumbnail temp file a crash left behind, for the delete paths. A file
/// that is already gone is not worth a warning.
pub fn remove_image_files(project_id: &str, image_id: &str, filename: &str) {
    let mut paths: Vec<PathBuf> = [
        image_file_path(project_id, image_id, filename),
        thumbnail_file_path(project_id, image_id),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some(dir) = paths.first().and_then(|p| p.parent()) {
        let prefix = format!("{image_id}.thumb.");
        if let Ok(entries) = std::fs::read_dir(dir) {
            paths.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".tmp"))
            }));
        }
    }
    for path in paths {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("Failed to remove image file {}: {e}", path.display());
            }
        }
    }
}
