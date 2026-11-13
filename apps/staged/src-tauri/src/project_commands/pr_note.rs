//! The project note a pull request's description is saved as.

use crate::store::{self, Store};

/// The note written beside a repo that was attached for a pull request:
/// `PR #<n>: <title>`, holding the pull request's description. The desktop
/// forms write it with a second command once the rows exist. A caller with
/// no form has the shared bodies write it instead. Not persisted as such:
/// it is an argument, and what is stored is an ordinary project note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrNote {
    pub title: String,
    pub content: String,
}

impl PrNote {
    /// The note for pull request `number`, titled the way the desktop forms
    /// title it.
    pub(crate) fn for_pull_request(number: u64, title: &str, body: &str) -> Self {
        Self {
            title: format!("PR #{number}: {title}"),
            content: body.to_string(),
        }
    }

    /// Save the note in `project_id`. Called once the project's rows are
    /// written, so a failure is logged and not returned: reporting it would
    /// tell the caller nothing was created.
    pub(super) fn write(&self, store: &Store, project_id: &str, log_tag: &str) {
        let note = store::ProjectNote::new(project_id, &self.title, &self.content);
        if let Err(e) = store.create_project_note(&note) {
            log::warn!(
                "[{log_tag}] failed to save the note '{}' in project {project_id}: {e}",
                self.title
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_request_note_is_titled_like_the_forms_title_it() {
        let note = PrNote::for_pull_request(42, "Fix login", "Why it broke.");
        assert_eq!(note.title, "PR #42: Fix login");
        assert_eq!(note.content, "Why it broke.");
    }

    #[test]
    fn writing_saves_one_project_note() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::named("alpha");
        store.create_project(&project).unwrap();

        PrNote::for_pull_request(42, "Fix login", "Why it broke.").write(
            &store,
            &project.id,
            "test",
        );

        let notes = store.list_project_notes(&project.id).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "PR #42: Fix login");
        assert_eq!(notes[0].content, "Why it broke.");
    }

    /// The rows the note belongs beside are already written, so a note that
    /// cannot be saved must not turn the whole call into an error.
    #[test]
    fn a_note_that_cannot_be_saved_is_not_an_error() {
        let store = Store::in_memory().unwrap();
        PrNote::for_pull_request(42, "Fix login", "").write(&store, "no-such-project", "test");
    }
}
