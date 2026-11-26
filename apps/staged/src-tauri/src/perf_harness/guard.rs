//! What must hold before the harness hooks switch on.

use std::path::PathBuf;

use super::{DIST_ENV, ROOT_ENV, TOKEN_ENV};

pub(super) struct Config {
    pub(super) port: u16,
    pub(super) dist: PathBuf,
    pub(super) token: String,
}

/// The configuration, or why the app must not run with the hooks on.
pub(super) fn check(port: &str) -> Result<Config, String> {
    let port: u16 = port
        .parse()
        .map_err(|_| format!("{} is not a port: {port:?}", super::PORT_ENV))?;
    let root = required(ROOT_ENV)?;
    let root = std::fs::canonicalize(&root).map_err(|e| format!("{ROOT_ENV} {root:?}: {e}"))?;
    let data_dir = crate::paths::data_dir().ok_or("no home directory")?;
    inside(&root, &data_dir)?;
    let token = required(TOKEN_ENV)?;
    if token.len() < 16 {
        return Err(format!("{TOKEN_ENV} is shorter than 16 characters"));
    }
    Ok(Config {
        port,
        dist: PathBuf::from(required(DIST_ENV)?),
        token,
    })
}

fn required(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set"))
}

/// The data directory does not exist yet on a first run, so its nearest
/// existing ancestor is what is resolved: a symlink anywhere on the way out
/// of `root` is caught there.
fn inside(root: &std::path::Path, data_dir: &std::path::Path) -> Result<(), String> {
    let existing = data_dir
        .ancestors()
        .find(|path| path.exists())
        .ok_or("the data directory has no existing ancestor")?;
    let resolved = std::fs::canonicalize(existing).map_err(|e| format!("{existing:?}: {e}"))?;
    if resolved.starts_with(root) {
        Ok(())
    } else {
        Err(format!(
            "the data directory {data_dir:?} (resolved through {resolved:?}) is not inside {root:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::inside;

    #[test]
    fn a_data_directory_under_the_root_passes_and_one_outside_it_does_not() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let home = root_path.join("home");
        std::fs::create_dir(&home).unwrap();
        assert!(inside(&root_path, &home.join(".staged")).is_ok());

        let elsewhere = tempfile::tempdir().unwrap();
        assert!(inside(&root_path, &elsewhere.path().join(".staged")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_root_does_not_pass() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let link = root_path.join("home");
        std::os::unix::fs::symlink(elsewhere.path(), &link).unwrap();
        assert!(inside(&root_path, &link.join(".staged")).is_err());
    }
}
