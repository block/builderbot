//! Caller-provided process environment snapshots for doctor subprocesses.

use std::borrow::Cow;
use std::collections::HashMap;
use std::process::Command;

/// A full environment snapshot captured by a caller.
///
/// When supplied to doctor APIs, subprocesses start from this snapshot instead
/// of inheriting the current process environment. `None` preserves the legacy
/// inherited/minimal environment behavior at each call site.
///
/// A snapshot can also carry per-check overlays (see
/// [`DoctorEnv::with_check_overlay`]): variables applied on top of `vars` for
/// one check's subprocesses only. Staged uses them for the environment
/// variables a user configures per agent in Doctor settings, so the agent's
/// auth probe and version probe see what a session launch of that agent sees.
#[derive(Debug, Clone, Default)]
pub struct DoctorEnv {
    /// The variables, in application order. A name may appear more than once
    /// — [`for_check`](Self::for_check) appends a check's overlay after the
    /// snapshot rather than replacing in place — and the **last pair for a
    /// name wins**. [`get`](Self::get) scans from the end, and
    /// [`apply_doctor_env`] and `build_shell_command` in `lib.rs` call
    /// `Command::env` in order, where a later call overwrites an earlier
    /// one, so all three agree. Anything that reorders or deduplicates this
    /// list must keep the last occurrence.
    pub vars: Vec<(String, String)>,
    /// Per-check variables, keyed by check id, applied after `vars` for that
    /// check (see [`DoctorEnv::for_check`]). Private so `new` stays the one
    /// constructor; the overlay for a check is reached through `for_check`.
    check_overlays: HashMap<String, Vec<(String, String)>>,
}

impl DoctorEnv {
    pub fn new(vars: Vec<(String, String)>) -> Self {
        Self {
            vars,
            check_overlays: HashMap::new(),
        }
    }

    /// Add variables that apply to `check_id`'s subprocesses only, after
    /// `vars`, so they win over the snapshot the way a per-agent variable wins
    /// over the inherited environment when the agent is launched. Calling this
    /// twice for one check appends; a later pair for the same name wins.
    pub fn with_check_overlay(
        mut self,
        check_id: impl Into<String>,
        overlay: Vec<(String, String)>,
    ) -> Self {
        if !overlay.is_empty() {
            self.check_overlays
                .entry(check_id.into())
                .or_default()
                .extend(overlay);
        }
        self
    }

    /// The environment for `check_id`'s subprocesses: this snapshot with the
    /// check's overlay appended, so the overlay's values are the ones
    /// [`get`](Self::get) returns and the ones a command receives. Borrows
    /// when the check has no overlay, which is the common case.
    pub fn for_check(&self, check_id: &str) -> Cow<'_, Self> {
        match self.check_overlays.get(check_id) {
            Some(overlay) => {
                let mut vars = self.vars.clone();
                vars.extend(overlay.iter().cloned());
                Cow::Owned(Self::new(vars))
            }
            None => Cow::Borrowed(self),
        }
    }

    /// The value `key` would have in a subprocess: the last pair with that
    /// name in [`vars`](Self::vars), which is the one `Command::env` leaves
    /// in place when the list is applied in order (see the field doc).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Start `command` from `env` alone. Applied in order, so a repeated name
/// resolves to its last pair, as [`DoctorEnv::get`] does.
pub(crate) fn apply_doctor_env(command: &mut Command, env: Option<&DoctorEnv>) {
    let Some(env) = env else {
        return;
    };
    command.env_clear();
    for (key, value) in &env.vars {
        command.env(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn check_overlay_applies_to_that_check_only_and_wins_over_the_snapshot() {
        let env = DoctorEnv::new(pairs(&[("KEY", "from snapshot"), ("PATH", "/usr/bin")]))
            .with_check_overlay("ai-agent-claude", pairs(&[("KEY", "from doctor row")]))
            .with_check_overlay("ai-agent-claude", pairs(&[("EXTRA", "1")]))
            // An empty overlay registers nothing, so the check still borrows.
            .with_check_overlay("ai-agent-codex", Vec::new());

        let claude = env.for_check("ai-agent-claude");
        assert!(matches!(claude, Cow::Owned(_)));
        assert_eq!(claude.get("KEY"), Some("from doctor row"));
        assert_eq!(claude.get("EXTRA"), Some("1"));
        assert_eq!(claude.get("PATH"), Some("/usr/bin"));
        // The overlay is appended, so `apply_doctor_env` sets it last too.
        assert_eq!(
            claude.vars,
            pairs(&[
                ("KEY", "from snapshot"),
                ("PATH", "/usr/bin"),
                ("KEY", "from doctor row"),
                ("EXTRA", "1"),
            ])
        );
        // The overlaid environment carries no overlays of its own.
        assert!(matches!(
            claude.for_check("ai-agent-claude"),
            Cow::Borrowed(_)
        ));

        for other in ["ai-agent-codex", "git"] {
            let other_env = env.for_check(other);
            assert!(matches!(other_env, Cow::Borrowed(_)));
            assert_eq!(other_env.get("KEY"), Some("from snapshot"));
            assert_eq!(other_env.get("EXTRA"), None);
        }
        assert_eq!(env.get("KEY"), Some("from snapshot"));
    }

    #[test]
    fn get_returns_the_overlay_value_for_a_name_the_snapshot_already_has() {
        // The overlay is appended, not substituted, so `vars` holds the name
        // twice; `get` must return the later, overlaid pair — the same value
        // `Command::env` ends up with when the list is applied in order.
        let env = DoctorEnv::new(pairs(&[("KEY", "from snapshot"), ("OTHER", "kept")]))
            .with_check_overlay("ai-agent-claude", pairs(&[("KEY", "from doctor row")]));
        let claude = env.for_check("ai-agent-claude");
        assert_eq!(claude.get("KEY"), Some("from doctor row"));
        assert_eq!(claude.get("OTHER"), Some("kept"));
        assert_eq!(claude.get("MISSING"), None);
        assert_eq!(
            claude.vars.iter().filter(|(k, _)| k == "KEY").count(),
            2,
            "the overlay appends rather than replacing"
        );
        // Two overlays for one check: the later one wins too.
        let twice = env
            .clone()
            .with_check_overlay("ai-agent-claude", pairs(&[("KEY", "from a later row")]));
        assert_eq!(
            twice.for_check("ai-agent-claude").get("KEY"),
            Some("from a later row")
        );
    }

    #[test]
    fn apply_doctor_env_sets_the_overlaid_variables_last() {
        let env = DoctorEnv::new(pairs(&[("KEY", "from snapshot")]))
            .with_check_overlay("ai-agent-claude", pairs(&[("KEY", "from doctor row")]));
        let mut command = Command::new("/usr/bin/env");
        apply_doctor_env(&mut command, Some(&env.for_check("ai-agent-claude")));
        let key = command
            .get_envs()
            .find(|(k, _)| *k == "KEY")
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned());
        assert_eq!(key.as_deref(), Some("from doctor row"));
    }
}
