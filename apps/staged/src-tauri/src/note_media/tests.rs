use super::*;
use crate::store::{Branch, Note, Project, Session};

pub(crate) const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfixture";

pub(crate) struct Fixture {
    pub store: std::sync::Arc<Store>,
    pub project: Project,
    pub branch: Branch,
    pub session: Session,
    pub dir: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let store = std::sync::Arc::new(Store::in_memory().unwrap());
        let project = Project::new("test/note-media");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "media", "main");
        store.create_branch(&branch).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new_running("note media", dir.path());
        store.create_session(&session).unwrap();
        Self {
            store,
            project,
            branch,
            session,
            dir,
        }
    }

    pub fn ingest(&self, previous: &str, content: &str) -> IngestOutcome {
        ingest_note_media(
            &self.store,
            NoteScope {
                branch_id: Some(&self.branch.id),
                project_id: &self.project.id,
                previous_content: previous,
            },
            &self.session.id,
            None,
            self.dir.path(),
            content,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = crate::git::project_worktree_root_for(&self.project.id) {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[test]
fn preserves_examples_and_external_images_and_titles() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("shot.png"), PNG).unwrap();
    let source = "é ![caption](shot.png \"Title\")\n![web](https://example.com/a.png)\n```md\n![example](/missing.png)\n```\n~~~\n![example](/missing.png)\n~~~\n`![example](/missing.png)`\n\\![example](/missing.png)\n";
    let outcome = f.ingest("", source);
    assert_eq!(outcome.kept_ids.len(), 1);
    assert!(outcome.markdown.starts_with("é ![caption](staged-media://"));
    assert!(outcome
        .markdown
        .ends_with(&source[source.find(" \"Title\")").unwrap()..]));
}

#[test]
fn amendments_reuse_hashes_and_materialized_paths_and_sweep_after_save() {
    let f = Fixture::new();
    let path = f.dir.path().join("shot.png");
    std::fs::write(&path, PNG).unwrap();
    let first = f.ingest("", "![Before](shot.png)");
    let id = first.kept_ids.iter().next().unwrap().clone();
    let note = Note::new(&f.branch.id, "Title", &first.markdown).with_session(&f.session.id);
    f.store.create_note(&note).unwrap();
    assert!(f
        .store
        .list_images_for_branch(&f.branch.id)
        .unwrap()
        .is_empty());
    assert!(f
        .store
        .get_image_ids_for_session(&f.session.id)
        .unwrap()
        .is_empty());
    assert_eq!(
        f.store
            .list_all_images_for_branch(&f.branch.id)
            .unwrap()
            .len(),
        1
    );
    for source in [
        "![Again](shot.png)".to_owned(),
        note.content.clone(),
        format!("![Again](/tmp/staged-image-{id}.png)"),
    ] {
        let amended = f.ingest(&note.content, &source);
        assert_eq!(amended.kept_ids, HashSet::from([id.clone()]));
        assert!(amended.created_ids.is_empty());
    }
    let materialized = materialize_note_media(&f.store, &note.content, None);
    assert!(!materialized.contains("staged-media://"));
    let target = extract_media_refs(&materialized).remove(0).target;
    assert_eq!(std::fs::read(&target).unwrap(), PNG);
    assert_eq!(
        f.ingest(&note.content, &materialized).kept_ids,
        HashSet::from([id.clone()])
    );
    std::fs::remove_file(target).unwrap();
    let removed = f.ingest(&note.content, "No attachment now");
    assert!(
        f.store.get_image(&id).unwrap().is_some(),
        "ingest must not delete the old version's file"
    );
    f.store
        .update_note_title_and_content(&note.id, "Title", &removed.markdown, None, None)
        .unwrap();
    removed.finish(&f.store, &note.content, true);
    assert!(f.store.get_image(&id).unwrap().is_none());
}

#[test]
fn amendments_hash_previous_attachments_only_when_a_fresh_file_needs_deduplicating() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("shot.png"), PNG).unwrap();
    let first = f.ingest("", "![Before](shot.png)");
    assert_eq!(first.previous_files_hashed, 0);
    let id = first.kept_ids.iter().next().unwrap().clone();
    let stored_only = f.ingest(&first.markdown, &first.markdown);
    assert_eq!(stored_only.kept_ids, HashSet::from([id.clone()]));
    assert_eq!(stored_only.previous_files_hashed, 0);
    let temp_path = f.ingest(
        &first.markdown,
        &format!("![Again](/tmp/staged-image-{id}.png)"),
    );
    assert_eq!(temp_path.kept_ids, HashSet::from([id.clone()]));
    assert_eq!(temp_path.previous_files_hashed, 0);
    std::fs::write(f.dir.path().join("copy.png"), PNG).unwrap();
    let fresh = f.ingest(&first.markdown, "![Copy](copy.png)");
    assert_eq!(fresh.kept_ids, HashSet::from([id]));
    assert_eq!(fresh.previous_files_hashed, 1);
}

#[test]
fn rewritten_notes_reuse_attachments_by_source_basename_when_the_file_is_gone() {
    let f = Fixture::new();
    let shot = f.dir.path().join("Shot.PNG");
    std::fs::write(&shot, PNG).unwrap();
    let first = f.ingest("", &format!("![Shot]({})", shot.display()));
    let id = first.kept_ids.iter().next().unwrap().clone();
    std::fs::remove_file(&shot).unwrap();
    let rewritten = f.ingest(
        &first.markdown,
        &format!("Rewritten\n\n![Shot]({})", shot.display()),
    );
    assert_eq!(
        rewritten.markdown,
        format!("Rewritten\n\n{}", first.markdown)
    );
    assert_eq!(rewritten.kept_ids, HashSet::from([id.clone()]));
    assert!(rewritten.created_ids.is_empty());
    // A different basename, and a name only another scope saved, stay unavailable.
    let other = f.ingest(&first.markdown, "![Shot](/tmp/missing/other.png)");
    assert!(other.kept_ids.is_empty());
    assert!(other.markdown.contains("media file is unavailable"));
    let unrelated = f.ingest("", &format!("![Shot]({})", shot.display()));
    assert!(unrelated.kept_ids.is_empty());
    assert!(unrelated.markdown.contains("media file is unavailable"));
    assert!(f.store.get_image(&id).unwrap().is_some());
}

#[test]
fn rewrites_match_numbered_filenames_and_claim_each_attachment_once() {
    let f = Fixture::new();
    // A chat upload already owns `shot.png` on this branch, so the note's
    // attachment is stored as `shot 2.png`.
    let chat = files::store_media_file(
        &f.store,
        Some(&f.branch.id),
        &f.project.id,
        Some(&f.session.id),
        "shot.png",
        PNG,
        false,
    )
    .unwrap();
    let shot = f.dir.path().join("shot.png");
    std::fs::write(&shot, [PNG, b"note"].concat()).unwrap();
    let first = f.ingest("", &format!("![Shot]({})", shot.display()));
    let id = first.kept_ids.iter().next().unwrap().clone();
    assert_eq!(
        f.store.get_image(&id).unwrap().unwrap().filename,
        "shot 2.png"
    );
    std::fs::remove_file(&shot).unwrap();
    let rewritten = f.ingest(&first.markdown, &format!("![Shot]({})", shot.display()));
    assert_eq!(rewritten.markdown, first.markdown);
    assert_eq!(rewritten.kept_ids, HashSet::from([id.clone()]));
    assert!(!rewritten.kept_ids.contains(&chat.id));

    // Two screenshots with one basename from different directories.
    let before = f.dir.path().join("before").join("screen.png");
    let after = f.dir.path().join("after").join("screen.png");
    for (path, suffix) in [(&before, &b"before"[..]), (&after, &b"after"[..])] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, [PNG, suffix].concat()).unwrap();
    }
    let source = format!(
        "![Before]({})\n![After]({})",
        before.display(),
        after.display()
    );
    let pair = f.ingest("", &source);
    assert_eq!(pair.kept_ids.len(), 2);
    let stored: Vec<String> = extract_media_refs(&pair.markdown)
        .into_iter()
        .map(|r| r.target)
        .collect();
    assert_ne!(stored[0], stored[1]);
    std::fs::remove_dir_all(before.parent().unwrap()).unwrap();
    std::fs::remove_dir_all(after.parent().unwrap()).unwrap();
    let rewritten = f.ingest(&pair.markdown, &format!("Rewritten\n\n{source}"));
    assert_eq!(
        rewritten.markdown,
        format!("Rewritten\n\n{}", pair.markdown)
    );
    assert_eq!(rewritten.kept_ids, pair.kept_ids);
    assert!(rewritten.created_ids.is_empty());
    // A third reference to the same basename finds no unclaimed row.
    let third = f.dir.path().join("third").join("screen.png");
    let extra = f.ingest(
        &pair.markdown,
        &format!("{source}\n![After]({})", third.display()),
    );
    assert_eq!(extra.kept_ids, pair.kept_ids);
    assert_eq!(
        extra.markdown.matches("media file is unavailable").count(),
        1
    );
    // Identical alt texts leave the pair ambiguous rather than guessing.
    let same_alt = source
        .replace("![Before]", "![Shot]")
        .replace("![After]", "![Shot]");
    let ambiguous = f.ingest(&pair.markdown, &same_alt);
    assert!(ambiguous.kept_ids.is_empty());
    assert_eq!(
        ambiguous
            .markdown
            .matches("media file is unavailable")
            .count(),
        2
    );
}

#[test]
fn basename_fallback_only_covers_missing_sources() {
    let f = Fixture::new();
    let shot = f.dir.path().join("shot.png");
    std::fs::write(&shot, PNG).unwrap();
    let first = f.ingest("", "![Shot](shot.png)");
    let id = first.kept_ids.iter().next().unwrap().clone();
    // A new file at the same path that breaks the size cap is a new, bad file.
    std::fs::File::create(&shot)
        .unwrap()
        .set_len(files::MAX_IMAGE_SIZE + 1)
        .unwrap();
    let oversized = f.ingest(&first.markdown, "![Shot](shot.png)");
    assert!(oversized.kept_ids.is_empty());
    assert!(oversized.markdown.contains("exceeds 10 MB limit"));
    assert!(!oversized.markdown.contains(&id));
    std::fs::remove_file(&shot).unwrap();
    std::fs::create_dir(&shot).unwrap();
    let directory = f.ingest(&first.markdown, "![Shot](shot.png)");
    assert!(directory.kept_ids.is_empty());
    assert!(directory.markdown.contains("media must be a regular file"));
    std::fs::remove_dir(&shot).unwrap();
    let missing = f.ingest(&first.markdown, "![Shot](shot.png)");
    assert_eq!(missing.kept_ids, HashSet::from([id]));
}

#[test]
fn remote_basename_fallback_only_covers_missing_sources() {
    let f = Fixture::new();
    let remote = FakeRemote::default();
    let path = "/work/shot.png".to_owned();
    remote.files.borrow_mut().insert(path.clone(), PNG.to_vec());
    let ingest = |previous: &str, content: &str| {
        ingest_note_media_with(
            &f.store,
            NoteScope {
                branch_id: Some(&f.branch.id),
                project_id: &f.project.id,
                previous_content: previous,
            },
            &f.session.id,
            Some("ws"),
            Path::new("/work"),
            content,
            &remote,
        )
    };
    let first = ingest("", "![Shot](shot.png)");
    assert_eq!(first.kept_ids.len(), 1);
    let id = first.kept_ids.iter().next().unwrap().clone();
    // The script exits 2 for a directory at the old path; that must not read
    // as a missing source.
    remote.files.borrow_mut().remove(&path);
    remote.directories.borrow_mut().insert(path.clone());
    let directory = ingest(&first.markdown, "![Shot](shot.png)");
    assert!(directory.kept_ids.is_empty());
    assert!(directory
        .markdown
        .contains("remote media must be a regular file"));
    assert!(!directory.markdown.contains(&id));
    remote.directories.borrow_mut().clear();
    let missing = ingest(&first.markdown, "![Shot](shot.png)");
    assert_eq!(missing.kept_ids, HashSet::from([id]));
    assert!(missing.markdown.contains("staged-media://"));
}

#[test]
fn unique_filename_variants_follow_the_store_numbering() {
    use crate::store::images::is_unique_filename_variant;
    for (stored, matches) in [
        ("shot.png", true),
        ("shot 2.png", true),
        ("shot 10.png", true),
        ("shot 1.png", false),
        ("shot  2.png", false),
        ("shot 2.PNG", false),
        ("shot2.png", false),
        ("shot 2.png.png", false),
        ("shot 2a.png", false),
        ("other.png", false),
    ] {
        assert_eq!(
            is_unique_filename_variant(stored, "shot.png"),
            matches,
            "{stored}"
        );
    }
    assert!(is_unique_filename_variant("a.b 3.png", "a.b.png"));
    assert!(is_unique_filename_variant("noext 2", "noext"));
    assert!(!is_unique_filename_variant("noext 2.png", "noext"));
}

/// Interprets exactly the commands the transfer, size check, and source read
/// issue, over an in-memory file map, so the staging and rename steps and the
/// read script's exit paths are observable.
#[derive(Default)]
pub(crate) struct FakeRemote {
    pub files: std::cell::RefCell<HashMap<String, Vec<u8>>>,
    pub directories: std::cell::RefCell<HashSet<String>>,
    pub commands: std::cell::RefCell<Vec<String>>,
    pub fail_writes: std::cell::Cell<bool>,
}

impl RemoteMedia for FakeRemote {
    fn exec_output(
        &self,
        _workspace: &str,
        args: &[&str],
    ) -> Result<crate::blox::WsExecOutput, String> {
        let ["sh", "-c", files::REMOTE_READ_SCRIPT, "staged-note-media", target, cwd, cap] = args
        else {
            panic!("unexpected remote command {args:?}");
        };
        let path = if target.starts_with('/') {
            (*target).to_owned()
        } else {
            format!("{cwd}/{target}")
        };
        let output = |stdout: Vec<u8>, stderr: &str, success: bool| {
            Ok(crate::blox::WsExecOutput {
                stdout,
                stderr: stderr.as_bytes().to_vec(),
                success,
            })
        };
        if self.directories.borrow().contains(&path) {
            return output(Vec::new(), files::REMOTE_NOT_REGULAR, false);
        }
        match self.files.borrow().get(&path) {
            Some(bytes) => {
                let cap: usize = cap.parse().unwrap();
                output(bytes[..bytes.len().min(cap)].to_vec(), "", true)
            }
            None => output(Vec::new(), "", false),
        }
    }

    fn exec(&self, _workspace: &str, args: &[&str]) -> Result<String, String> {
        use base64::Engine;
        let mut files = self.files.borrow_mut();
        match args {
            ["sh", "-c", "wc -c < \"$1\"", _, path] => files
                .get(*path)
                .map(|bytes| bytes.len().to_string())
                .ok_or_else(|| "exit status 1".to_owned()),
            ["rm", "-f", path] => {
                files.remove(*path);
                Ok(String::new())
            }
            ["sh", "-c", command] => {
                self.commands.borrow_mut().push((*command).to_owned());
                if self.fail_writes.get() {
                    return Err("exit status 1".into());
                }
                let (encoded, rest) = command
                    .strip_prefix("echo '")
                    .and_then(|rest| rest.split_once("' | base64 -d "))
                    .unwrap();
                let (redirect, rest) = rest.split_once(" '").unwrap();
                let (part, rest) = rest.split_once('\'').unwrap();
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap();
                let staged = files.entry(part.to_owned()).or_default();
                if redirect == ">" {
                    staged.clear();
                }
                staged.extend(decoded);
                if let Some(rest) = rest.strip_prefix(" && mv -f '") {
                    let (source, rest) = rest.split_once("' '").unwrap();
                    let destination = rest.strip_suffix('\'').unwrap();
                    let bytes = files.remove(source).unwrap();
                    files.insert(destination.to_owned(), bytes);
                } else {
                    assert!(rest.is_empty(), "{command}");
                }
                Ok(String::new())
            }
            other => panic!("unexpected remote command {other:?}"),
        }
    }
}

#[test]
fn remote_materialization_skips_matching_copies_and_publishes_atomically() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("shot.png"), PNG).unwrap();
    let outcome = f.ingest("", "![a](shot.png) ![b](shot.png)");
    let id = outcome.kept_ids.iter().next().unwrap();
    let remote = FakeRemote::default();
    let path = format!("/tmp/staged-image-{id}.png");
    let expected = format!("![a]({path}) ![b]({path})");
    let first = materialize_note_media_with(&f.store, &outcome.markdown, Some("ws"), &remote);
    assert_eq!(first, expected);
    {
        let commands = remote.commands.borrow();
        assert_eq!(commands.len(), 1);
        assert!(
            !commands[0].contains(&format!("> '{path}'")),
            "{}",
            commands[0]
        );
        assert!(
            commands[0].contains(&format!("> '{path}.")) && commands[0].contains(".part'"),
            "{}",
            commands[0]
        );
        assert!(
            commands[0].ends_with(&format!(".part' '{path}'")),
            "{}",
            commands[0]
        );
    }
    assert_eq!(
        *remote.files.borrow(),
        HashMap::from([(path.clone(), PNG.to_vec())]),
        "only the published file remains"
    );
    let second = materialize_note_media_with(&f.store, &outcome.markdown, Some("ws"), &remote);
    assert_eq!(second, expected);
    assert_eq!(
        remote.commands.borrow().len(),
        1,
        "matching copy is not re-sent"
    );
    remote.files.borrow_mut().insert(path.clone(), vec![0]);
    materialize_note_media_with(&f.store, &outcome.markdown, Some("ws"), &remote);
    assert_eq!(
        remote.commands.borrow().len(),
        2,
        "truncated copy is re-sent"
    );
    assert_eq!(remote.files.borrow()[&path], PNG);
    remote.files.borrow_mut().clear();
    remote.fail_writes.set(true);
    let failed = materialize_note_media_with(&f.store, &outcome.markdown, Some("ws"), &remote);
    assert!(failed.contains("media unavailable"));
    assert!(!failed.contains("staged-media://"));
    assert!(
        remote.files.borrow().is_empty(),
        "a failed transfer leaves neither the final path nor a staging file"
    );
}

#[test]
fn missing_and_mislabelled_media_become_safe_placeholders() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("fake.png"), b"not a png").unwrap();
    let missing_id = uuid::Uuid::new_v4();
    let outcome = f.ingest(
        "",
        &format!("![a](fake.png) ![b](missing.png) ![c](staged-media://{missing_id}.png)"),
    );
    assert!(outcome.kept_ids.is_empty());
    assert!(outcome.markdown.contains("file signature does not match"));
    assert!(outcome.markdown.contains("media file is unavailable"));
    assert!(outcome.markdown.contains("stored media is unavailable"));
    assert!(!outcome.markdown.contains("!["));
    assert!(!outcome.markdown.contains("staged-media://"));
    assert!(refs::unavailable("/<img>.png", "unavailable").contains("\\<img\\>"));
}

#[test]
fn accepts_relative_absolute_file_urls_and_angle_paths_with_titles() {
    let f = Fixture::new();
    let path = f.dir.path().join("a (shot).PNG");
    std::fs::write(&path, PNG).unwrap();
    let url = reqwest::Url::from_file_path(&path).unwrap();
    let source = format!(
        "![a](<a (shot).PNG> 'caption')\n![b](<{}>)\n![c]({url})",
        path.display()
    );
    let outcome = f.ingest("", &source);
    assert_eq!(outcome.kept_ids.len(), 1);
    assert_eq!(outcome.markdown.matches("staged-media://").count(), 3);
    assert!(outcome.markdown.contains(".png> 'caption')"));
}

#[test]
fn validates_magic_and_caps_by_kind_without_reading_oversized_files() {
    use files::*;
    for (name, data) in [
        ("a.png", PNG),
        ("a.jpeg", &b"\xff\xd8\xffx"[..]),
        ("a.gif", &b"GIF89a"[..]),
        ("a.webp", &b"RIFF1234WEBP"[..]),
        ("a.mp4", &b"\0\0\0\x18ftypisom\0\0\0\0"[..]),
        ("a.mov", &b"\0\0\0\x18ftypqt  \0\0\0\0"[..]),
        ("a.webm", &b"\x1a\x45\xdf\xa3xxxxwebm"[..]),
    ] {
        assert!(validate_media(name, data, true).is_ok(), "{name}");
    }
    assert!(validate_media("fake.mp4", b"\0\0\0\x18ftypavif\0\0\0\0", true).is_err());
    assert!(validate_media("fake.webm", b"\x1a\x45\xdf\xa3matroska", true).is_err());
    assert!(size_limit("a.mp4", false).is_err());
    assert_eq!(size_limit("a.png", true).unwrap(), MAX_IMAGE_SIZE);
    assert_eq!(size_limit("a.mov", true).unwrap(), MAX_VIDEO_SIZE);
    let f = Fixture::new();
    for (name, cap) in [("large.png", MAX_IMAGE_SIZE), ("large.mp4", MAX_VIDEO_SIZE)] {
        std::fs::File::create(f.dir.path().join(name))
            .unwrap()
            .set_len(cap + 1)
            .unwrap();
        let result = f.ingest("", &format!("![Large]({name})"));
        assert!(result.markdown.contains("exceeds"));
        assert!(result.kept_ids.is_empty());
    }
}

#[test]
fn failed_note_save_rolls_back_new_files_but_keeps_previous_media() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("shot.png"), PNG).unwrap();
    let first = f.ingest("", "![a](shot.png)");
    let old_id = first.kept_ids.iter().next().unwrap().clone();
    let note = Note::new(&f.branch.id, "Title", &first.markdown);
    f.store.create_note(&note).unwrap();
    std::fs::write(f.dir.path().join("new.png"), [PNG, b"new"].concat()).unwrap();
    let next = f.ingest(&note.content, "![b](new.png)");
    let new_id = next.kept_ids.iter().next().unwrap().clone();
    next.finish(&f.store, &note.content, false);
    assert!(f.store.get_image(&new_id).unwrap().is_none());
    assert!(f.store.get_image(&old_id).unwrap().is_some());
}

#[test]
fn shared_note_refs_survive_deletion_until_last_reference_is_removed() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("shot.png"), PNG).unwrap();
    let outcome = f.ingest("", "![a](shot.png)");
    let note = Note::new(&f.branch.id, "One", &outcome.markdown);
    let other = Note::new(&f.branch.id, "Two", &outcome.markdown);
    f.store.create_note(&note).unwrap();
    f.store.create_note(&other).unwrap();
    let id = outcome.kept_ids.iter().next().unwrap();
    f.store.delete_note(&note.id).unwrap();
    cleanup_unreferenced(&f.store, media_ids(&note.content));
    assert!(f.store.get_image(id).unwrap().is_some());
    f.store.delete_note(&other.id).unwrap();
    cleanup_unreferenced(&f.store, media_ids(&other.content));
    assert!(f.store.get_image(id).unwrap().is_none());
}

#[test]
fn materialization_missing_files_and_remote_video_caps_are_readable() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("shot.png"), PNG).unwrap();
    let outcome = f.ingest("", "![a](shot.png)");
    let id = outcome.kept_ids.iter().next().unwrap();
    let image = f.store.get_image(id).unwrap().unwrap();
    std::fs::remove_file(
        crate::store::images::image_file_path(&image.project_id, id, &image.filename).unwrap(),
    )
    .unwrap();
    let materialized = materialize_note_media(&f.store, &outcome.markdown, None);
    assert!(materialized.contains("media unavailable"));
    assert!(!materialized.contains("staged-media://"));
    let video = Image::new(
        Some(&f.branch.id),
        &f.project.id,
        "large.mp4",
        "video/mp4",
        files::MAX_REMOTE_VIDEO_SIZE + 1,
        false,
    )
    .with_session(&f.session.id);
    f.store.create_image(&video).unwrap();
    let content = format!("![Clip](staged-media://{}.mp4)", video.id);
    let materialized = materialize_note_media(&f.store, &content, Some("unused-workspace"));
    assert!(materialized.contains("20 MB remote video transfer limit"));
    assert!(materialized.contains("large\\.mp4"));
    assert!(materialized.starts_with("Clip — "));
    assert!(materialized.contains(&video.size_bytes.to_string()));
}

#[test]
fn desktop_protocol_seeks_and_handles_head_and_unsatisfiable_ranges() {
    use tauri::http::{Request, StatusCode};
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("shot.png"), PNG).unwrap();
    let outcome = f.ingest("", "![a](shot.png)");
    let id = outcome.kept_ids.iter().next().unwrap();
    let uri = format!("staged-media://localhost/{id}.png");
    let response = serving::response(
        &f.store,
        Request::builder()
            .uri(&uri)
            .header("Range", "bytes=2-5")
            .body(vec![])
            .unwrap(),
    );
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.body(), &PNG[2..6]);
    assert_eq!(
        response.headers()["content-range"],
        format!("bytes 2-5/{}", PNG.len())
    );
    assert_eq!(response.headers()["content-type"], "image/png");
    let response = serving::response(
        &f.store,
        Request::builder()
            .uri(&uri)
            .header("Range", "bytes=999-")
            .body(vec![])
            .unwrap(),
    );
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    let response = serving::response(
        &f.store,
        Request::builder()
            .uri(&uri)
            .method("HEAD")
            .body(vec![])
            .unwrap(),
    );
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.body().is_empty());
    assert_eq!(response.headers()["content-length"], PNG.len().to_string());
    assert!(serving::lookup(&f.store, "../file.png").is_err());
    assert!(serving::lookup(&f.store, &format!("{id}.mp4")).is_err());
}

#[test]
fn copied_chat_media_is_owned_by_the_note_and_original_survives_cleanup() {
    let f = Fixture::new();
    let original = files::store_media_file(
        &f.store,
        Some(&f.branch.id),
        &f.project.id,
        Some(&f.session.id),
        "chat.png",
        PNG,
        false,
    )
    .unwrap();
    f.store
        .add_session_message_with_images(
            &f.session.id,
            crate::store::MessageRole::User,
            "Screenshot",
            std::slice::from_ref(&original.id),
        )
        .unwrap();
    let outcome = f.ingest("", &format!("![a](/tmp/staged-image-{}.png)", original.id));
    assert!(!outcome.kept_ids.contains(&original.id));
    cleanup_unreferenced(&f.store, HashSet::from([original.id.clone()]));
    assert!(f.store.get_image(&original.id).unwrap().is_some());
}

#[test]
fn project_note_deletion_collects_child_media_and_removes_files() {
    let f = Fixture::new();
    let image = files::store_media_file(
        &f.store,
        None,
        &f.project.id,
        Some(&f.session.id),
        "project.png",
        PNG,
        true,
    )
    .unwrap();
    let parent = crate::store::ProjectNote::new(
        &f.project.id,
        "Parent",
        &format!("![a](staged-media://{}.png)", image.id),
    );
    f.store.create_project_note(&parent).unwrap();
    std::fs::write(f.dir.path().join("child.png"), PNG).unwrap();
    let outcome = f.ingest("", "![Child](child.png)");
    let mut child = Note::new(&f.branch.id, "Child", &outcome.markdown);
    child.parent_project_note_id = Some(parent.id.clone());
    f.store.create_note(&child).unwrap();
    let ids = project_note_media_ids(&f.store, &parent.id).unwrap();
    assert_eq!(ids.len(), 2);
    f.store.delete_project_note(&parent.id).unwrap();
    cleanup_unreferenced(&f.store, ids.clone());
    for id in ids {
        assert!(f.store.get_image(&id).unwrap().is_none());
    }
    assert!(
        !crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
            .unwrap()
            .exists()
    );
}

#[test]
fn parser_handles_multiline_inline_syntax_and_ignores_code_and_reference_images() {
    let source = "![line\ncaption](path(1).png\n  \"Title\")\n`code\n![skip](hidden.png)`\n![reference][ref]\n[ref]: not-ingested.png\n````md\n```\n![skip](hidden.png)\n````\n    ![skip](hidden.png)\n";
    let refs = extract_media_refs(source);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].target, "path(1).png");
    assert_eq!(&source[refs[0].destination.clone()], "path(1).png");
    assert_eq!(
        &source[refs[0].span.clone()],
        "![line\ncaption](path(1).png\n  \"Title\")"
    );
}

#[test]
fn percent_encoded_and_home_relative_paths_resolve() {
    let f = Fixture::new();
    let source = f.dir.path().join("a shot.png");
    std::fs::write(&source, PNG).unwrap();
    let home = dirs::home_dir().unwrap();
    let home_relative = format!(
        "~/{}{}",
        "../".repeat(home.components().count() - 1),
        source
            .to_string_lossy()
            .trim_start_matches('/')
            .replace(' ', "%20")
    );
    let result = f.ingest("", &format!("![a](a%20shot.png)\n![b]({home_relative})"));
    assert_eq!(result.kept_ids.len(), 1);
    assert_eq!(result.markdown.matches("staged-media://").count(), 2);
}

#[test]
fn fenced_examples_in_quotes_and_lists_are_not_ingested() {
    for source in [
        "> ```md\n> ![example](hidden.png)\n> ```\n![real](real.png)",
        "- ```md\n  ![example](hidden.png)\n  ```\n![real](real.png)",
        "1. ~~~md\n   ![example](hidden.png)\n   ~~~\n![real](real.png)",
    ] {
        let refs = extract_media_refs(source);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].target, "real.png");
    }
}

/// Expected counts match Marked (`gfm: true, breaks: true`), the app's renderer.
fn assert_image_counts(cases: &[(&str, usize)]) {
    for (source, expected) in cases {
        assert_eq!(extract_media_refs(source).len(), *expected, "{source:?}");
    }
}

#[test]
fn images_inside_lists_are_ingested() {
    assert_image_counts(&[
        ("1. Step\n\n    ![S](p)", 1),
        ("1. Step\n    ![S](p)", 1),
        ("- Step\n\n    ![S](p)", 1),
        ("- a\n  - b\n\n      ![S](p)", 1),
        ("-   a\n\n      ![S](p)", 1),
        ("10. a\n\n    ![S](p)", 1),
        ("100. a\n\n     ![S](p)", 1),
        ("> - a\n>\n>     ![S](p)", 1),
        ("- Step\n\n\t![S](p)", 1),
        ("- a\n- b\n\n    ![S](p)", 1),
        ("- a\n\n    ![S](p)\n        ![T](p)", 2),
        ("1. Step\r\n\r\n    ![S](p)\r\n", 1),
    ]);
    let source = "1. Step\n\n    ![Shot](/tmp/shot.png)\n";
    let refs = extract_media_refs(source);
    assert_eq!(&source[refs[0].destination.clone()], "/tmp/shot.png");
}

#[test]
fn indented_code_examples_stay_code_relative_to_containers() {
    assert_image_counts(&[
        ("- Step\n\n      ![S](p)", 0),
        ("- a\n\n        ![S](p)", 0),
        ("- a\n    - b\n\n          ![S](p)", 0),
        ("1.     ![S](p)", 0),
        ("Para\n\n    ![S](p)", 0),
        ("- a\n\nPara\n\n    ![S](p)", 0),
        ("> - a\n\n    ![S](p)", 0),
        (">     ![S](p)", 0),
        ("> a\n>\n>     ![S](p)", 0),
        ("- a\n\n\t    ![S](p)", 0),
    ]);
}

#[test]
fn paragraph_continuations_and_lazy_lines_follow_marked() {
    assert_image_counts(&[
        ("Here it is:\n    ![S](p)", 1),
        ("> a\n>     ![S](p)", 1),
        ("- a\nb\n\n    ![S](p)", 1),
        ("- a\n===\n\n    ![S](p)", 1),
        ("- a\n# h\n\n    ![S](p)", 0),
        ("- a\n---\n\n    ![S](p)", 0),
        ("- a\n> q\n\n    ![S](p)", 0),
        ("- a\n```\n\n    ![S](p)", 0),
        ("# Title\n    ![S](p)", 0),
        ("---\n    ![S](p)", 0),
        ("Title\n===\n    ![S](p)", 0),
    ]);
}

/// A fence ends with its container. Blockquotes end when a line lacks their
/// prefix; Marked's list items absorb dedented lines unless a blank line
/// preceded them or they start a block, and a dedented ``` then opens a new
/// fence rather than closing the old one.
#[test]
fn fences_end_with_their_container_and_dedented_closers_reopen() {
    assert_image_counts(&[
        ("- item\n  ```\n  code\n```\nafter ![S](p)\n```\n", 0),
        ("> ```\n> code\n```\nafter ![S](p)\n```\n", 0),
        ("- a\n  ```\n  code\n```\n  after ![S](p)", 0),
        ("- a\n  ```\n  code\n~~~\nafter ![S](p)\n~~~", 0),
        ("- a\n\n  ```\n  code\n```\nafter ![S](p)\n```", 0),
        ("- ```\n  code\n  ```\nafter ![S](p)", 1),
        ("- a\n  ```\n  code\nlazy ![S](p)\n  ```\n", 0),
        ("> - a\n>   ```\n>   code\n> lazy ![S](p)\n>   ```", 0),
        ("- a\n  ```\n  code\n\n  more ![S](p)\n  ```\n", 0),
        ("- a\n  ```\n  code\n\nafter ![S](p)", 1),
        ("- a\n  ```\n  code\n  \nafter ![S](p)", 1),
        ("- a\n  ```\n  code\n\n  ```\nafter ![S](p)", 1),
        ("- a\n  ```\n  code\n# h\n![S](p)", 1),
        ("- a\n  ```\n  code\n- b\n![S](p)", 1),
        ("- a\n  ```\n  code\n---\n![S](p)", 1),
        ("- a\n  ```\n  code\n> q ![S](p)", 1),
        ("- a\n  ```\n  code\n<pre>\n![S](p)", 0),
        ("> ```\n> code\nlazy ![S](p)\n> ```\n", 1),
        ("> ```\n> code\n\n> more ![S](p)\n> ```\n", 1),
    ]);
}

/// HTML blocks (CommonMark 4.6 conditions 1, 2, and 6) render as literal
/// text, so their images must not be ingested.
#[test]
fn html_blocks_are_not_prose() {
    assert_image_counts(&[
        ("<pre>\n![S](p)\n</pre>", 0),
        ("<!-- ![S](p) -->", 0),
        ("<pre>\n![S](p)\n</PRE>\n![T](p)", 1),
        ("<pre>\n\n![S](p)\n\n</pre>\n![T](p)", 1),
        ("<pre>\n![S](p)\n</pre>![T](p)", 0),
        ("<pre>\n![S](p)\n</pre> tail\n![T](p)", 1),
        ("<Pre>![S](p)</pre> ![T](p)", 0),
        ("<pre\n![S](p)\n</pre>\n![T](p)", 1),
        ("<pre/>\n![S](p)\n</pre>\n![T](p)", 2),
        ("<prex>\n![S](p)\n\n![T](p)", 2),
        ("</pre>\n![S](p)\n\n![T](p)", 2),
        ("<script>\n![S](p)\n</script>", 0),
        ("<style>\n![S](p)", 0),
        ("<textarea>\n![S](p)\n</textarea>\n![T](p)", 1),
        // Only the opening tag's own closer ends a raw block; with none, it
        // runs to the end of the document.
        ("<pre>\n![S](p)\n</script>\n![T](p)", 0),
        ("<pre>\n![S](p)\n</script>\n![T](p)\n</pre>\n![U](p)", 1),
        ("<pre>\n![S](p)\n</pre>\n</script>\n![T](p)", 1),
        ("<script>\n![S](p)\n</pre>\n</script>\n![T](p)", 1),
        ("<textarea>\n![S](p)\n</pre>\n![T](p)", 0),
        ("<style>\n![S](p)\n</STYLE>\n![T](p)", 1),
        ("<pre>\n![S](p)\n</pre\n![T](p)", 0),
        ("<pre>\n![S](p)\n</prex>\n![T](p)", 0),
        ("- <pre>\n  ![S](p)\n  </script>\n![T](p)", 0),
        ("<pre>\n```\n![S](p)\n</pre>\n![T](p)", 1),
        ("```\n<pre>\n```\n![S](p)", 1),
        ("<!--\n![S](p)\n-->\n![T](p)", 1),
        ("<!-->\n![S](p)\n-->\n![T](p)", 2),
        ("<!-- a --> ![S](p)\n![T](p)", 1),
        ("<div>\n![S](p)\n</div>\n\n![T](p)", 1),
        ("<div>\n\n![S](p)\n</div>", 1),
        ("<div>![S](p)\n\n![T](p)", 1),
        ("<div class=\"x\">\n![S](p)\n\n![T](p)", 1),
        ("<div\n![S](p)", 0),
        ("<div/>\n![S](p)", 0),
        ("</div>\n![S](p)", 0),
        ("<DIV>\n![S](p)", 0),
        ("<table>\n![S](p)\n</table>", 0),
        ("<h1>\n![S](p)\n\n![T](p)", 1),
        ("<p>\n![S](p)\n\n![T](p)", 1),
        ("Para\n<pre>\n![S](p)\n</pre>", 0),
        ("Para\n<div>\n![S](p)\n</div>", 0),
        ("Para\n<!-- ![S](p) -->\n![T](p)", 1),
        (" <pre>\n![S](p)", 0),
        ("    <pre>\n![S](p)", 1),
        ("\t<pre>\n![S](p)", 1),
        ("- <pre>\n  ![S](p)\n  </pre>", 0),
        ("- <div>\n  ![S](p)\n\n  ![T](p)", 1),
        ("- <div>\n  ![S](p)\n![T](p)", 0),
        ("- <pre>\n  ![S](p)\n![T](p)\n  </pre>\n![U](p)", 1),
        ("- <pre>\n  ![S](p)\n\n![T](p)", 1),
        ("> <pre>\n> ![S](p)\n![T](p)\n> </pre>\n![U](p)", 2),
        ("> <pre>\n> ![S](p)\n\n![T](p)", 1),
        ("> <div>\n> ![S](p)\n![T](p)\n\n![U](p)", 2),
        ("> <!-- ![S](p) -->\n> ![T](p)", 1),
        ("1. <!--\n   ![S](p)\n   -->\n   ![T](p)", 1),
        ("- a\n<pre>\n![S](p)\n</pre>", 0),
        ("- a\n<div>\n![S](p)\n</div>", 0),
        ("- a\n  <div>\n  ![S](p)\n\n  ![T](p)", 1),
        ("- a\n  <!--\n  x\nlazy ![S](p)\n  -->\n![T](p)", 1),
    ]);
}

#[test]
fn indented_list_images_are_ingested_without_changing_indentation() {
    let f = Fixture::new();
    std::fs::write(f.dir.path().join("shot.png"), PNG).unwrap();
    let outcome = f.ingest("", "1. Step\n\n    ![Screenshot](shot.png)");
    assert_eq!(outcome.kept_ids.len(), 1);
    let id = outcome.kept_ids.iter().next().unwrap();
    assert_eq!(
        outcome.markdown,
        format!("1. Step\n\n    ![Screenshot](staged-media://{id}.png)")
    );
    assert!(media_ids(&outcome.markdown).contains(id));
}
