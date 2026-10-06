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
