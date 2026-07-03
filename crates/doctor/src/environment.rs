//! Caller-provided process environment snapshots for doctor subprocesses.

use std::process::Command;

/// A full environment snapshot captured by a caller.
///
/// When supplied to doctor APIs, subprocesses start from this snapshot instead
/// of inheriting the current process environment. `None` preserves the legacy
/// inherited/minimal environment behavior at each call site.
#[derive(Debug, Clone, Default)]
pub struct DoctorEnv {
    pub vars: Vec<(String, String)>,
}

impl DoctorEnv {
    pub fn new(vars: Vec<(String, String)>) -> Self {
        Self { vars }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

pub(crate) fn apply_doctor_env(command: &mut Command, env: Option<&DoctorEnv>) {
    let Some(env) = env else {
        return;
    };
    command.env_clear();
    for (key, value) in &env.vars {
        command.env(key, value);
    }
}
