//! Note-diff seeder: a project note that has been revised three times, and
//! one whose revision dropped its media, for trying note diffs in a scratch
//! data directory.
//!
//! An `#[ignore]`d test, so it never runs in CI. It opens (or creates)
//! `data.db` in `STAGED_NOTE_DIFF_SEED_DIR`, which must be an absolute path
//! that does not lead into `~/.staged` (through `..` or a symlink either),
//! and writes through the code the app itself runs:
//!
//! - a project named "Note diffs demo" with no repository, by the store
//!   writes behind the app's own "new project" form, and the project's
//!   directory, which the app makes at the same moment;
//! - one project note, created with the first of four fixture revisions and
//!   then revised to each of the others with
//!   `Store::update_project_note_title_and_content`, the write a session's
//!   note goes through. Each revision keeps the text it replaces, so the
//!   note ends with three versions;
//! - a second project note, written and revised once the same way, whose
//!   first text embeds a screenshot and a recording that the revision
//!   dropped.
//!
//! The note viewer compares a note with its newest version only, so the last
//! revision is the one the change bar shows. It holds one of each change:
//! a reworded sentence, an edited diagram, a removed paragraph, a changed
//! list item, a changed table row and an added section, in that order down
//! the note. The earlier revisions are there so the note has more than one
//! version to choose the newest from; the first of them also retitles it.
//!
//! The second note is there for what a version shows when its media is gone.
//! Media is deleted once no note or chat embeds it, and a note's versions do
//! not count: a revision that drops an image takes the file with it, and the
//! version goes on naming it. The seeder stores no media at all, which leaves
//! the fixture's two references exactly as that clean-up leaves them, with no
//! row and no file, and the viewer draws a placeholder for each.
//!
//! Run from `apps/staged/src-tauri`, with no Staged running on that
//! directory (it would not hear of the change until restarted):
//!
//! ```sh
//! STAGED_NOTE_DIFF_SEED_DIR=/tmp/staged-note-diffs/home/.staged \
//!   cargo test --lib note_versions_seed -- --ignored --nocapture
//! ```
//!
//! Then start a development build with `STAGED_DATA_DIR` set to the same
//! directory; `docs/note-diffs.md` has the whole recipe. A directory that
//! already holds the demo project is refused: seed a fresh one, or delete
//! the project in the app first.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::board_load_bench::guarded_scratch_dir;
use crate::project_commands::{create_project_impl, CreateProjectRequest};
use crate::store::{ProjectNote, Store};

const SEED_DIR_ENV: &str = "STAGED_NOTE_DIFF_SEED_DIR";
const PROJECT_NAME: &str = "Note diffs demo";

/// The note's text over time, oldest first, each as a note draft: the title
/// as a `# ` heading on the first line and the body below it. The frontend
/// reads the last two as well (`noteDiffSeedFixtures.component.test.ts`), to
/// hold them to the changes the module docs list.
const REVISIONS: [&str; 4] = [
    include_str!("note_versions_seed/revision-1.md"),
    include_str!("note_versions_seed/revision-2.md"),
    include_str!("note_versions_seed/revision-3.md"),
    include_str!("note_versions_seed/revision-4.md"),
];

/// The second note's text, before and after the revision that dropped its
/// screenshot (for a sentence) and its recording, and added a finding. The
/// ids its first text names are of media that is not stored.
const MEDIA_REVISIONS: [&str; 2] = [
    include_str!("note_versions_seed/media-revision-1.md"),
    include_str!("note_versions_seed/media-revision-2.md"),
];

/// A revision's `(title, body)`, split as `write_note` splits a draft.
fn revision(draft: &str) -> (String, String) {
    crate::note_draft::split_title(draft).expect("a fixture is a note draft")
}

struct Seeded {
    db_path: PathBuf,
    project_id: String,
    note_id: String,
    /// The note whose previous version names media that is gone.
    media_note_id: String,
}

fn seed_dir() -> PathBuf {
    let dir = PathBuf::from(
        std::env::var_os(SEED_DIR_ENV)
            .unwrap_or_else(|| panic!("set {SEED_DIR_ENV} to a scratch data directory")),
    );
    let installed = crate::paths::default_data_dir().expect("home directory");
    guarded_seed_dir(&dir, &installed).unwrap_or_else(|refusal| panic!("{refusal}"))
}

/// The directory to seed, created, or the reason it is refused: a path that
/// is not absolute, as the app ignores such a `STAGED_DATA_DIR`, and one
/// that leads into `installed_data_dir`.
///
/// That is `~/.staged` whatever `STAGED_DATA_DIR` says. The override is how
/// the scratch directory gets used, so the directory it names is not the one
/// to keep out of.
fn guarded_seed_dir(dir: &Path, installed_data_dir: &Path) -> Result<PathBuf, String> {
    if !dir.is_absolute() {
        return Err(format!(
            "{SEED_DIR_ENV} must be an absolute path, as STAGED_DATA_DIR must: {}",
            dir.display()
        ));
    }
    guarded_scratch_dir(SEED_DIR_ENV, dir, &[installed_data_dir.to_path_buf()])
}

/// Where `path` would be were `data_dir` the data directory. `path` comes
/// from a `paths` helper, which builds it under the directory this process
/// would use; only its place inside that directory is kept.
fn in_data_dir(data_dir: &Path, path: &Path) -> PathBuf {
    let own = crate::paths::data_dir().expect("home directory");
    let inside = path
        .strip_prefix(&own)
        .expect("a path under the data directory");
    data_dir.join(inside)
}

/// Write the demo project and its note into `data_dir`'s database.
fn seed(data_dir: &Path) -> Result<Seeded, String> {
    let db_path = in_data_dir(data_dir, &crate::paths::db_path().expect("home directory"));
    let store = Arc::new(Store::new(&db_path).map_err(|e| e.to_string())?);

    // The rows the "new project" form writes when no repository is picked.
    // A project without one is listed and opens like any other, and its
    // notes are shown above the place its branches would be.
    let request = CreateProjectRequest {
        name: PROJECT_NAME.to_string(),
        ..Default::default()
    };
    let project = create_project_impl(Arc::clone(&store), request, false)
        .map_err(|e| format!("{} is already seeded? {e}", db_path.display()))?
        .project;
    // And the directory it makes beside them (`start_project_setup`), which
    // a session started on the project runs in.
    let project_dir = crate::git::project_worktree_root_for(&project.id)
        .map_err(|e| e.to_string())
        .map(|root| in_data_dir(data_dir, &root))?;
    std::fs::create_dir_all(&project_dir)
        .map_err(|e| format!("cannot create {}: {e}", project_dir.display()))?;

    let [first, later @ ..] = REVISIONS;
    let note_id = seed_note(&store, &project.id, first, &later)?;
    let [first, later @ ..] = MEDIA_REVISIONS;
    let media_note_id = seed_note(&store, &project.id, first, &later)?;

    Ok(Seeded {
        db_path,
        project_id: project.id,
        note_id,
        media_note_id,
    })
}

/// A project note created with the draft `first` and revised to each of
/// `later` in turn, by the write a session's note goes through.
fn seed_note(
    store: &Store,
    project_id: &str,
    first: &str,
    later: &[&str],
) -> Result<String, String> {
    let (title, body) = revision(first);
    let note = ProjectNote::new(project_id, &title, &body);
    store
        .create_project_note(&note)
        .map_err(|e| e.to_string())?;
    for draft in later {
        let (title, body) = revision(draft);
        store
            .update_project_note_title_and_content(&note.id, &title, &body, None, None)
            .map_err(|e| e.to_string())?;
    }
    Ok(note.id)
}

/// What the app reads to show the project, list its note and diff it, from
/// the database as it is on disk: a new connection, as the app's will be.
fn assert_readable_as_the_app_reads_it(seeded: &Seeded) {
    let store = Store::new(&seeded.db_path).unwrap();

    // The projects grid and the sidebar: `list_projects`.
    let projects = store.list_projects().unwrap();
    let project = projects
        .iter()
        .find(|project| project.id == seeded.project_id)
        .expect("the project is listed");
    assert_eq!(project.name, PROJECT_NAME);

    // The project's notes: `list_project_notes`. A row is one the user can
    // open when it reads as done, which a note with a body and no session
    // does; an unread one is drawn as such.
    let notes = store
        .list_project_notes_with_status(&seeded.project_id)
        .unwrap();
    assert_eq!(notes.len(), 2, "the project has two notes");
    for (id, current) in [
        (&seeded.note_id, REVISIONS[3]),
        (&seeded.media_note_id, MEDIA_REVISIONS[1]),
    ] {
        let note = notes
            .iter()
            .find(|note| &note.id == id)
            .expect("the note is listed");
        let (current_title, current_body) = revision(current);
        assert_eq!(
            (&note.title, &note.content),
            (&current_title, &current_body)
        );
        assert_eq!(note.session_id, None);
        assert_eq!(note.session_status, None);
        assert!(note.completed_at.is_some());
        assert!(!note.is_read());
    }

    // The note viewer: `get_note_previous_version`, the newest of the three.
    let previous = store
        .get_previous_note_version(&seeded.note_id)
        .unwrap()
        .expect("the note has a previous version");
    let (previous_title, previous_body) = revision(REVISIONS[2]);
    assert_eq!(
        (&previous.title, &previous.content),
        (&previous_title, &previous_body)
    );
    let versions: i64 = store
        .hold_connection()
        .query_row(
            "SELECT COUNT(*) FROM note_versions WHERE note_id = ?1",
            [&seeded.note_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(versions, 3);

    // The second note's one version names media its revision dropped, and
    // none of it is stored: what the viewer has to show a placeholder for.
    let previous = store
        .get_previous_note_version(&seeded.media_note_id)
        .unwrap()
        .expect("the media note has a previous version");
    let (_, previous_body) = revision(MEDIA_REVISIONS[0]);
    assert_eq!(previous.content, previous_body);
    let (_, current_body) = revision(MEDIA_REVISIONS[1]);
    let dropped = crate::note_media::refs::media_ids(&previous.content);
    assert_eq!(dropped.len(), 2);
    assert!(crate::note_media::refs::media_ids(&current_body).is_empty());
    for id in dropped {
        assert!(store.get_image(&id).unwrap().is_none());
    }
}

#[test]
#[ignore = "seeder: run by hand, see the module docs"]
fn note_versions_seed() {
    let dir = seed_dir();
    let seeded = seed(&dir).unwrap_or_else(|refusal| panic!("{refusal}"));
    assert_readable_as_the_app_reads_it(&seeded);

    let (title, _) = revision(REVISIONS[3]);
    println!("\nSeeded {}", seeded.db_path.display());
    println!("  project  {PROJECT_NAME}  ({})", seeded.project_id);
    println!("  note     {title}  ({})", seeded.note_id);
    println!("  versions 3 kept, the newest is the one the change bar compares with");
    let (title, _) = revision(MEDIA_REVISIONS[1]);
    println!("  note     {title}  ({})", seeded.media_note_id);
    println!("  versions 1 kept, naming a screenshot and a recording that are gone");
    println!(
        "\nStart Staged on it with STAGED_DATA_DIR={}",
        dir.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pikchr_validation::validate_pikchr_blocks;

    /// A stand-in home, canonical as macOS hands out temporary directories
    /// behind a symlink: `installed` plays `~/.staged`.
    fn sandbox() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let installed = root.join("installed");
        std::fs::create_dir(&installed).unwrap();
        (temp, root, installed)
    }

    #[test]
    fn a_fresh_directory_is_seeded_with_a_note_the_app_can_show_and_diff() {
        let (_temp, root, installed) = sandbox();
        let dir = guarded_seed_dir(&root.join("scratch/.staged"), &installed).unwrap();

        let seeded = seed(&dir).unwrap();

        assert_eq!(seeded.db_path, dir.join("data.db"));
        assert_readable_as_the_app_reads_it(&seeded);
        assert!(dir
            .join("workspaces/local/projects")
            .join(&seeded.project_id)
            .is_dir());
        assert_eq!(std::fs::read_dir(&installed).unwrap().count(), 0);
    }

    #[test]
    fn a_directory_seeded_before_is_refused_and_left_as_it_was() {
        let (_temp, root, installed) = sandbox();
        let dir = guarded_seed_dir(&root.join("scratch"), &installed).unwrap();
        let seeded = seed(&dir).unwrap();

        let refusal = seed(&dir).err().expect("a second seed is refused");

        assert!(refusal.contains("already seeded"), "{refusal}");
        assert_readable_as_the_app_reads_it(&seeded);
        assert_eq!(
            Store::new(&seeded.db_path)
                .unwrap()
                .list_projects()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn the_installed_data_directory_is_refused_and_nothing_is_written_to_it() {
        let (_temp, root, installed) = sandbox();
        std::fs::create_dir(root.join("elsewhere")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&installed, root.join("link")).unwrap();

        let mut inside = vec![
            installed.clone(),
            installed.join("scratch"),
            root.join("elsewhere/../installed/scratch"),
        ];
        if cfg!(unix) {
            inside.push(root.join("link/scratch"));
        }
        for dir in inside {
            let refusal = guarded_seed_dir(&dir, &installed).unwrap_err();

            assert!(
                refusal.contains(&format!("{SEED_DIR_ENV} must not be inside")),
                "{refusal}"
            );
        }
        assert_eq!(std::fs::read_dir(&installed).unwrap().count(), 0);
    }

    #[test]
    fn a_relative_directory_is_refused() {
        let (_temp, _root, installed) = sandbox();

        let refusal = guarded_seed_dir(Path::new("scratch/.staged"), &installed).unwrap_err();

        assert!(refusal.contains("must be an absolute path"), "{refusal}");
        assert!(!Path::new("scratch").exists());
    }

    /// The store keeps whatever it is handed, so nothing else would notice a
    /// fixture whose diagram does not draw.
    #[test]
    fn every_revision_is_a_note_whose_diagrams_parse() {
        for (index, draft) in REVISIONS.iter().chain(&MEDIA_REVISIONS).enumerate() {
            let (title, body) = revision(draft);
            assert!(!title.is_empty());

            let errors = validate_pikchr_blocks(&body);

            assert!(
                errors.is_empty(),
                "revision {}: line {}: {}",
                index + 1,
                errors[0].line_number,
                errors[0].message
            );
        }
        // The diagram is there to be edited by the last revision.
        for draft in &REVISIONS[2..] {
            assert_eq!(
                crate::pikchr_validation::extract_pikchr_blocks(draft).len(),
                1
            );
        }
    }
}
