//! Durable inline note attachments, using the existing images table and files.
//! These helper types are transient; no new persisted ownership/schema is needed.

mod blocks;
pub(crate) mod files;
mod materialize;
pub(crate) use materialize::materialize_note_media;
#[cfg(test)]
pub(crate) use materialize::{materialize_note_media_with, RemoteMedia};
pub(crate) mod refs;
pub(crate) mod serving;

use crate::store::{Image, Store};
use refs::{extract_media_refs, media_ids, stored_ref, unavailable};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(crate) struct NoteScope<'a> {
    pub branch_id: Option<&'a str>,
    pub project_id: &'a str,
    pub previous_content: &'a str,
}

pub(crate) struct IngestOutcome {
    pub markdown: String,
    pub kept_ids: HashSet<String>,
    created_ids: HashSet<String>,
    /// Previous attachments read back for deduplication; amendments that only
    /// repeat stored references should leave this at zero.
    #[cfg(test)]
    pub previous_files_hashed: usize,
}

/// Attachments the note referenced before this turn, within the same scope.
/// Rows are cheap; hashing their bytes (up to 100 MB each) is not, so that
/// waits until a fresh file actually needs deduplicating.
struct PreviousAttachments {
    images: Vec<Image>,
    /// Alt texts the previous content used for each attachment id.
    alts: HashMap<String, HashSet<String>>,
    hashes: Option<HashMap<(String, Vec<u8>), Image>>,
    files_hashed: usize,
}

impl PreviousAttachments {
    fn load(store: &Store, scope: &NoteScope<'_>) -> Self {
        let mut alts = HashMap::<String, HashSet<String>>::new();
        for reference in extract_media_refs(scope.previous_content) {
            if let Some((id, _)) = stored_ref(&reference.target) {
                alts.entry(id.to_owned())
                    .or_default()
                    .insert(reference.alt.clone());
            }
        }
        let images = alts
            .keys()
            .filter_map(|id| store.get_image(id).ok().flatten())
            .filter(|image| {
                image.project_id == scope.project_id
                    && image.branch_id.as_deref() == scope.branch_id
            })
            .collect();
        Self {
            images,
            alts,
            hashes: None,
            files_hashed: 0,
        }
    }

    fn contains(&self, id: &str) -> bool {
        self.alts.contains_key(id)
    }

    fn hashes(&mut self) -> &mut HashMap<(String, Vec<u8>), Image> {
        let (images, files_hashed) = (&self.images, &mut self.files_hashed);
        self.hashes.get_or_insert_with(|| {
            let mut hashes = HashMap::new();
            for image in images {
                if let Ok(path) = crate::store::images::image_file_path(
                    &image.project_id,
                    &image.id,
                    &image.filename,
                ) {
                    if let Ok(bytes) = files::read_local(&path, files::MAX_VIDEO_SIZE) {
                        *files_hashed += 1;
                        hashes.insert(
                            (image.mime_type.clone(), Sha256::digest(&bytes).to_vec()),
                            image.clone(),
                        );
                    }
                }
            }
            hashes
        })
    }

    /// A rewriting turn repeats the agent's original source paths, which may
    /// be gone by now. The attachment saved from that path keeps its basename
    /// (with a normalized extension), so match on that, then on alt text.
    fn by_filename(&self, filename: &str, alt: &str) -> Option<Image> {
        let normalized = Path::new(filename)
            .with_extension(files::extension(filename))
            .to_string_lossy()
            .into_owned();
        let matches: Vec<&Image> = self
            .images
            .iter()
            .filter(|image| image.filename == normalized)
            .collect();
        let narrowed: Vec<&Image> = match matches.len() {
            0 => return None,
            1 => matches,
            _ => matches
                .into_iter()
                .filter(|image| self.alts[&image.id].contains(alt))
                .collect(),
        };
        match narrowed.as_slice() {
            [image] => Some((*image).clone()),
            _ => None,
        }
    }
}

impl IngestOutcome {
    /// Sweep only after the note update commits. If persistence fails, roll
    /// back newly created attachments and leave the old note's files intact.
    pub fn finish(self, store: &Store, previous_content: &str, saved: bool) {
        let candidates = if saved {
            media_ids(previous_content)
                .difference(&self.kept_ids)
                .cloned()
                .chain(self.created_ids)
                .collect()
        } else {
            self.created_ids
        };
        cleanup_unreferenced(store, candidates);
    }
}

pub(crate) fn ingest_note_media(
    store: &Store,
    scope: NoteScope<'_>,
    session_id: &str,
    workspace_name: Option<&str>,
    working_dir: &Path,
    markdown: &str,
) -> IngestOutcome {
    let mut outcome = IngestOutcome {
        markdown: markdown.to_owned(),
        kept_ids: HashSet::new(),
        created_ids: HashSet::new(),
        #[cfg(test)]
        previous_files_hashed: 0,
    };
    let mut previous = PreviousAttachments::load(store, &scope);
    // Process in document order for filename suffixes/deduplication; apply edits
    // backwards so offsets continue to refer to the original source.
    let mut edits = Vec::new();
    let mut targets = HashMap::<String, Image>::new();
    for reference in extract_media_refs(markdown) {
        let target = &reference.target;
        if target
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
            || target
                .get(..7)
                .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
            || target.starts_with("//")
        {
            continue;
        }
        let result = (|| {
            if let Some(image) = targets.get(target) {
                return Ok(image.clone());
            }
            let local_target = files::local_target(target)?;
            let existing = stored_ref(target).or_else(|| {
                // Local materialization uses the OS temp directory (on macOS
                // it is not /tmp). Only our exact temp roots get this shortcut.
                let path = Path::new(&local_target);
                let parent = path.parent()?;
                if parent != Path::new("/tmp") && parent != std::env::temp_dir() {
                    return None;
                }
                refs::parse_media_file(path.file_name()?.to_str()?.strip_prefix("staged-image-")?)
            });
            if let Some((id, ext)) = existing {
                let image = store
                    .get_image(id)
                    .map_err(|e| e.to_string())?
                    .ok_or("stored media is unavailable")?;
                if files::mime_for_extension(ext) != Some(image.mime_type.as_str()) {
                    return Err("media format does not match stored file".into());
                }
                // A branch can move independently of other branches or project
                // notes. Copy cross-scope refs so subsequent moves are safe.
                if image.project_id == scope.project_id
                    && image.branch_id.as_deref() == scope.branch_id
                    && (previous.contains(id)
                        || store.media_is_referenced(id).map_err(|e| e.to_string())?)
                {
                    return Ok(image);
                }
                let path = crate::store::images::image_file_path(
                    &image.project_id,
                    &image.id,
                    &image.filename,
                )?;
                let bytes = files::read_local(&path, files::size_limit(&image.filename, true)?)?;
                return save_candidate(
                    store,
                    &scope,
                    session_id,
                    &image.filename,
                    &bytes,
                    &mut previous,
                    &mut outcome.created_ids,
                );
            }
            if target.starts_with("staged-media:") {
                return Err("invalid stored media reference".into());
            }
            let filename = local_target
                .rsplit('/')
                .next()
                .ok_or("invalid media path")?;
            let cap = files::size_limit(filename, true)?;
            let bytes = match files::read_candidate(&local_target, workspace_name, working_dir, cap)
            {
                Ok(bytes) => bytes,
                // Prefer the attachment saved from this path over a placeholder
                // that would also have finish() delete it.
                Err(reason) => return previous.by_filename(filename, &reference.alt).ok_or(reason),
            };
            save_candidate(
                store,
                &scope,
                session_id,
                filename,
                &bytes,
                &mut previous,
                &mut outcome.created_ids,
            )
        })();
        match result {
            Ok(image) => {
                edits.push((
                    reference.destination,
                    format!(
                        "staged-media://{}.{}",
                        image.id,
                        files::extension(&image.filename)
                    ),
                ));
                outcome.kept_ids.insert(image.id.clone());
                targets.insert(target.clone(), image);
            }
            Err(reason) => {
                log::warn!("Note media {target:?}: {reason}");
                edits.push((reference.span, unavailable(target, &reason)));
            }
        }
    }
    for (span, replacement) in edits.into_iter().rev() {
        outcome.markdown.replace_range(span, &replacement);
    }
    #[cfg(test)]
    {
        outcome.previous_files_hashed = previous.files_hashed;
    }
    outcome
}

fn save_candidate(
    store: &Store,
    scope: &NoteScope<'_>,
    session_id: &str,
    filename: &str,
    bytes: &[u8],
    previous: &mut PreviousAttachments,
    created: &mut HashSet<String>,
) -> Result<Image, String> {
    let mime = files::validate_media(filename, bytes, true)?;
    let key = (mime.to_owned(), Sha256::digest(bytes).to_vec());
    let hashes = previous.hashes();
    if let Some(image) = hashes.get(&key) {
        return Ok(image.clone());
    }
    let image = files::store_media_file(
        store,
        scope.branch_id,
        scope.project_id,
        Some(session_id),
        filename,
        bytes,
        true,
    )?;
    created.insert(image.id.clone());
    hashes.insert(key, image.clone());
    Ok(image)
}

/// The note has already been removed/updated when this runs. Protect shared
/// refs and ordinary timeline/chat attachments from destructive cleanup.
pub(crate) fn cleanup_unreferenced(store: &Store, ids: HashSet<String>) {
    for id in ids {
        let result = (|| {
            if store.media_is_referenced(&id).map_err(|e| e.to_string())? {
                return Ok(());
            }
            let Some(image) = store.get_image(&id).map_err(|e| e.to_string())? else {
                return Ok(());
            };
            if image.session_id.is_none()
                || store
                    .image_has_chat_references(&id)
                    .map_err(|e| e.to_string())?
            {
                return Ok(());
            }
            files::delete_media_file(store, &image)
        })();
        if let Err(e) = result {
            log::warn!("Cannot clean up note media {id}: {e}");
        }
    }
}

pub(crate) fn project_note_media_ids(store: &Store, id: &str) -> Result<HashSet<String>, String> {
    let mut ids = store
        .get_project_note(id)
        .map_err(|e| e.to_string())?
        .map(|n| media_ids(&n.content))
        .unwrap_or_default();
    for note in store.list_child_notes(id).map_err(|e| e.to_string())? {
        ids.extend(media_ids(&note.content));
    }
    Ok(ids)
}

#[cfg(test)]
pub(crate) mod tests;
