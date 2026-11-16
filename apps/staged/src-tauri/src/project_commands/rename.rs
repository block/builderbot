//! Renaming a project: a metadata-only store write.

use crate::store::{self, Store};

/// Rename a project, shared by the `rename_project` Tauri command and the
/// web transport.
///
/// A metadata-only write: the `projects.name` column changes and the store
/// publishes `project-changed`. Branch names, worktrees, Blox workspaces and
/// repo branch names keep whatever was inferred from the old name when they
/// were created, so nothing on disk moves. It is not gated on an in-flight
/// transfer: a transfer rewrites `location`, which this write never carries.
pub(crate) fn rename_project_impl(
    store: &Store,
    project_id: &str,
    name: &str,
) -> Result<store::Project, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Project name is required".to_string());
    }
    store
        .rename_project(project_id, trimmed)
        .map_err(|e| e.to_string())?;
    store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_project_impl_trims_and_stores_the_name() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();

        let renamed = rename_project_impl(&store, &project.id, "  Renamed  ").unwrap();

        assert_eq!(renamed.name, "Renamed");
        assert_eq!(renamed.id, project.id);
        assert_eq!(
            store.get_project(&project.id).unwrap().unwrap().name,
            "Renamed"
        );
    }

    #[test]
    fn rename_project_impl_rejects_a_blank_name_without_writing() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();

        let err = rename_project_impl(&store, &project.id, "   \t\n").unwrap_err();

        assert_eq!(err, "Project name is required");
        assert_eq!(
            store.get_project(&project.id).unwrap().unwrap().name,
            project.name
        );
    }

    #[test]
    fn rename_project_impl_surfaces_a_missing_project() {
        let store = Store::in_memory().unwrap();

        let err = rename_project_impl(&store, "missing", "Renamed").unwrap_err();

        assert!(err.contains("Project not found"), "{err}");
    }
}
