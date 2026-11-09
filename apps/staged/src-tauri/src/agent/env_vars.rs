//! Per-agent environment variables (Settings → Doctor → Environment variables).
//!
//! The frontend preferences store persists, for each agent id, an ordered list
//! of `{ name, value, enabled }` entries under [`AGENT_ENV_VARS_KEY`] in
//! `preferences.json`. This module reads that list directly from the file —
//! mirroring `branch-prefix` and `diagram-subsession-config` — and turns it
//! into the `KEY=VALUE` pairs a local agent launch adds to the process
//! environment. Disabled rows and rows whose name isn't a usable variable
//! name are ignored; a missing or malformed key reads as "no variables", and
//! a malformed entry is skipped on its own, as the frontend skips it.
//!
//! # Storage
//!
//! Values are stored in plaintext in `preferences.json`, alongside every
//! other preference, by design: the obvious use is API keys, but the file is
//! already private to the local user, the agent reads the value out of its
//! own environment regardless, and the preferences file is also served to
//! web-UI clients. Encrypting it here would only move the problem to key
//! management without changing who can read the secret. The Doctor UI says
//! so in the field description, so the user chooses knowing this. Revisit if
//! preferences ever leave the local machine (sync, export, remote sessions).
//! The variables apply to local launches only: a remote session's extra
//! environment reaches the `sq blox acp` proxy, not the workstation agent.
//!
//! # Precedence
//!
//! The driver applies `extra_env` last in every local spawn path: over the
//! captured shell snapshot when it spawns the binary directly, and on the
//! `exec` line (via `/usr/bin/env`) when it falls back to the login shell —
//! `$SHELL -ils` by default, or `$SHELL -l -c` running that same line
//! through `/bin/sh -c` for a shell known not to be POSIX (fish, xonsh, the
//! C shells; the line travels in an environment variable, never quoted into
//! the command) — so an `export` in the user's rc files can't shadow a Doctor
//! value there either. The flip side is that a row named after something the
//! launcher
//! itself sets — `PATH`, `GOOSE_SEARCH_PATHS` (see
//! `acp_tools::apply_managed_tools_env`) — replaces Staged's value for that
//! agent and everything it spawns. The Doctor UI warns on those names
//! (`LAUNCHER_RESERVED_NAMES` in the frontend) but doesn't refuse them.
//!
//! # Doctor
//!
//! Doctor's own probes of an agent — `auth status`, the version probe — and
//! its login fix run with the same variables, laid over Doctor's shared
//! snapshot for that agent's check only (`doctor::agent_env_for_check`, a
//! `DoctorEnv` per-check overlay), so the panel can't call an agent
//! unauthenticated against the key its sessions launch with. Those probes go
//! through `zsh -l -c`, which reads `.zshenv`/`.zprofile` after the seed and
//! has no exec-line reapplication, so an export there can still shadow a row
//! for the probe alone; `.zshrc` isn't read by `-l -c`.

/// Preferences-store key: `Record<agentId, AgentEnvVar[]>`. Kept in sync with
/// `AGENT_ENV_VARS_STORE_KEY` in the frontend preferences store.
pub(crate) const AGENT_ENV_VARS_KEY: &str = "agent-env-vars";

/// One configured variable for an agent. `enabled` lets a row be switched
/// off without being deleted; an entry written without it reads as enabled.
/// `value` is held and persisted as plaintext (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentEnvVar {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) enabled: bool,
}

impl AgentEnvVar {
    /// One stored entry, or `None` when it isn't an object with a string
    /// `name`. Reads the other fields the way `normalizeAgentEnvVars` in the
    /// frontend does — a missing or non-string `value` as empty, a missing or
    /// non-boolean `enabled` as enabled — so both sides agree on which rows
    /// exist.
    fn from_json(entry: &serde_json::Value) -> Option<Self> {
        let object = entry.as_object()?;
        let name = object.get("name")?.as_str()?.to_string();
        let value = object
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let enabled = object
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        Some(Self {
            name,
            value,
            enabled,
        })
    }
}

/// Whether `name` can be set as an environment variable: non-empty, with no
/// `=`, whitespace or NUL. Matches `validateEnvVarName` in the frontend, which
/// shows the user an inline error for anything this rejects.
pub(crate) fn is_valid_env_var_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('=')
        && !name.contains('\0')
        && !name.chars().any(char::is_whitespace)
}

/// The configured entries for `provider_id` from a parsed `preferences.json`.
/// A missing key, an agent with no list, or a value that isn't a list reads
/// as empty. Within a list, each entry is read on its own: one that isn't an
/// object with a string `name` is skipped, with a warning in the log, and the
/// rest are kept. This matches `normalizeAgentEnvVars` in the frontend, so
/// the rows the user sees as enabled in Doctor are the rows that get applied;
/// dropping the whole list on one bad entry would silently launch the agent
/// without variables the UI shows as valid.
pub(crate) fn agent_env_vars_from_preferences(
    json: &serde_json::Value,
    provider_id: &str,
) -> Vec<AgentEnvVar> {
    let Some(entries) = json
        .get(AGENT_ENV_VARS_KEY)
        .and_then(|by_agent| by_agent.get(provider_id))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let parsed = AgentEnvVar::from_json(entry);
            if parsed.is_none() {
                // Index only: the entry may hold a secret in some other field.
                log::warn!(
                    "Skipping malformed environment variable entry {index} for agent {provider_id}: \
                     expected an object with a string name"
                );
            }
            parsed
        })
        .collect()
}

/// The `KEY=VALUE` pairs to apply from `entries`, in order: enabled rows with a
/// usable name only. A name listed twice keeps its last enabled value, so the
/// result has one pair per name and the order of first appearance.
pub(crate) fn enabled_env_pairs(entries: &[AgentEnvVar]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for entry in entries {
        if !entry.enabled || !is_valid_env_var_name(&entry.name) || entry.value.contains('\0') {
            continue;
        }
        match pairs.iter_mut().find(|(name, _)| *name == entry.name) {
            Some((_, value)) => *value = entry.value.clone(),
            None => pairs.push((entry.name.clone(), entry.value.clone())),
        }
    }
    pairs
}

/// Merge the user's per-agent variables with a session's own extra variables
/// into the list handed to the driver's `with_extra_env`, which applies them
/// after the inherited environment in every spawn path (see the module
/// docs) so both override it.
/// Staged's per-session variables come last: a commit session's
/// `GIT_AUTHOR_DATE`, say, is a requirement of that session, not a default
/// the user should be able to shadow from Doctor settings.
pub(crate) fn merge_agent_env(
    agent_env: Vec<(String, String)>,
    session_env: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut merged = agent_env;
    for (name, value) in session_env {
        match merged.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, existing_value)) => *existing_value = value,
            None => merged.push((name, value)),
        }
    }
    merged
}

/// Read and parse `preferences.json` once. `None` on any failure — no store
/// path, missing file, unparseable JSON — which every caller treats as "no
/// variables configured", so the agent launches as before. Callers that need
/// several agents' variables (Doctor's overlays) read once and derive each
/// list from the same value, so they all describe one snapshot of the file
/// even if the UI writes it mid-way.
pub(crate) fn read_preferences() -> Option<serde_json::Value> {
    let path = crate::preferences_store_path_buf()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<serde_json::Value>(&contents).ok()
}

/// The enabled variables configured for `provider_id` in a parsed
/// `preferences.json`, as `KEY=VALUE` pairs ready for the driver.
pub(crate) fn agent_env_from_preferences(
    json: &serde_json::Value,
    provider_id: &str,
) -> Vec<(String, String)> {
    enabled_env_pairs(&agent_env_vars_from_preferences(json, provider_id))
}

/// Read the enabled variables configured for `provider_id` from the
/// preferences store. Any failure — missing file, unparseable JSON, absent or
/// malformed key — yields no variables, so the agent launches as before.
/// Called at session launch, for pikchr sub-agents, and by Doctor for the
/// agent's login fix; Doctor's check overlays go through
/// [`read_preferences`] and [`agent_env_from_preferences`] instead so every
/// agent's list comes from one read.
pub(crate) fn read_agent_env(provider_id: &str) -> Vec<(String, String)> {
    read_preferences()
        .map(|json| agent_env_from_preferences(&json, provider_id))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn var(name: &str, value: &str, enabled: bool) -> AgentEnvVar {
        AgentEnvVar {
            name: name.to_string(),
            value: value.to_string(),
            enabled,
        }
    }

    #[test]
    fn deserializes_entries_with_defaults() {
        let json = json!({
            "agent-env-vars": {
                "claude": [
                    { "name": "FOO", "value": "bar", "enabled": true },
                    { "name": "OFF", "value": "x", "enabled": false },
                    { "name": "LEGACY" }
                ]
            }
        });
        assert_eq!(
            agent_env_vars_from_preferences(&json, "claude"),
            vec![
                var("FOO", "bar", true),
                var("OFF", "x", false),
                var("LEGACY", "", true),
            ]
        );
    }

    #[test]
    fn missing_key_agent_or_malformed_list_reads_as_empty() {
        assert!(agent_env_vars_from_preferences(&json!({}), "claude").is_empty());
        let other_agent = json!({ "agent-env-vars": { "goose": [{ "name": "A", "value": "1" }] } });
        assert!(agent_env_vars_from_preferences(&other_agent, "claude").is_empty());
        let not_a_list = json!({ "agent-env-vars": { "claude": { "name": "A" } } });
        assert!(agent_env_vars_from_preferences(&not_a_list, "claude").is_empty());
        let bad_entry = json!({ "agent-env-vars": { "claude": [{ "value": "no name" }] } });
        assert!(agent_env_vars_from_preferences(&bad_entry, "claude").is_empty());
        let wrong_type = json!({ "agent-env-vars": ["FOO=bar"] });
        assert!(agent_env_vars_from_preferences(&wrong_type, "claude").is_empty());
    }

    #[test]
    fn skips_only_the_malformed_entries_and_keeps_the_rest() {
        // Mirrors the `normalizeAgentEnvVars` frontend test: one bad entry
        // must not drop the agent's whole list, or rows the UI shows as valid
        // would silently go unapplied.
        let json = json!({
            "agent-env-vars": {
                "claude": [
                    { "name": "A", "value": "1" },
                    { "value": "no name" },
                    "FOO=bar",
                    null,
                    { "name": 5, "value": "numeric name" },
                    { "name": "B", "value": 5, "enabled": "yes" },
                    { "name": "C", "enabled": false }
                ]
            }
        });
        assert_eq!(
            agent_env_vars_from_preferences(&json, "claude"),
            vec![var("A", "1", true), var("B", "", true), var("C", "", false)]
        );
    }

    #[test]
    fn enabled_pairs_skip_disabled_and_invalid_names() {
        let entries = vec![
            var("KEEP", "1", true),
            var("OFF", "2", false),
            var("", "3", true),
            var("HAS=EQUALS", "4", true),
            var("HAS SPACE", "5", true),
            var("NUL\0", "6", true),
            var("NUL_VALUE", "a\0b", true),
            var("EMPTY_VALUE", "", true),
        ];
        assert_eq!(
            enabled_env_pairs(&entries),
            vec![
                ("KEEP".to_string(), "1".to_string()),
                ("EMPTY_VALUE".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn enabled_pairs_keep_order_and_let_a_later_duplicate_win() {
        let entries = vec![
            var("A", "first", true),
            var("B", "b", true),
            var("A", "second", true),
            var("A", "disabled third", false),
        ];
        assert_eq!(
            enabled_env_pairs(&entries),
            vec![
                ("A".to_string(), "second".to_string()),
                ("B".to_string(), "b".to_string()),
            ]
        );
    }

    #[test]
    fn merge_lets_session_env_override_agent_env() {
        let agent = vec![
            ("USER_VAR".to_string(), "user".to_string()),
            ("GIT_AUTHOR_DATE".to_string(), "user-date".to_string()),
        ];
        let session = vec![
            ("GIT_AUTHOR_DATE".to_string(), "session-date".to_string()),
            ("GIT_COMMITTER_DATE".to_string(), "session-date".to_string()),
        ];
        assert_eq!(
            merge_agent_env(agent, session),
            vec![
                ("USER_VAR".to_string(), "user".to_string()),
                ("GIT_AUTHOR_DATE".to_string(), "session-date".to_string()),
                ("GIT_COMMITTER_DATE".to_string(), "session-date".to_string()),
            ]
        );
    }

    #[test]
    fn merge_with_nothing_configured_is_the_session_env() {
        let session = vec![("A".to_string(), "1".to_string())];
        assert_eq!(merge_agent_env(Vec::new(), session.clone()), session);
        assert!(merge_agent_env(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn valid_names() {
        assert!(is_valid_env_var_name("FOO"));
        assert!(is_valid_env_var_name("lower_case1"));
        assert!(is_valid_env_var_name("1LEADING_DIGIT"));
        assert!(!is_valid_env_var_name(""));
        assert!(!is_valid_env_var_name("A=B"));
        assert!(!is_valid_env_var_name("A B"));
        assert!(!is_valid_env_var_name("A\tB"));
        assert!(!is_valid_env_var_name("A\0"));
    }
}
