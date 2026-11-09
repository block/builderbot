//! Per-agent environment variables (Settings → Doctor → Environment variables).
//!
//! The frontend preferences store persists, for each agent id, an ordered list
//! of `{ name, value, enabled }` entries under [`AGENT_ENV_VARS_KEY`] in
//! `preferences.json`. This module reads that list directly from the file —
//! mirroring `branch-prefix` and `diagram-subsession-config` — and turns it
//! into the `KEY=VALUE` pairs a local agent launch adds to the process
//! environment. Disabled rows and rows whose name isn't a usable variable
//! name are ignored; a missing or malformed key reads as "no variables".

use serde::Deserialize;

/// Preferences-store key: `Record<agentId, AgentEnvVar[]>`. Kept in sync with
/// `AGENT_ENV_VARS_STORE_KEY` in the frontend preferences store.
pub(crate) const AGENT_ENV_VARS_KEY: &str = "agent-env-vars";

/// One configured variable for an agent. `enabled` lets a row be switched
/// off without being deleted; an entry written without it reads as enabled.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentEnvVar {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) value: String,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
}

fn default_enabled() -> bool {
    true
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
/// A missing key, an agent with no list, or a malformed list reads as empty;
/// one malformed entry drops the whole list for that agent rather than
/// guessing at it.
pub(crate) fn agent_env_vars_from_preferences(
    json: &serde_json::Value,
    provider_id: &str,
) -> Vec<AgentEnvVar> {
    json.get(AGENT_ENV_VARS_KEY)
        .and_then(|by_agent| by_agent.get(provider_id))
        .and_then(|entries| serde_json::from_value(entries.clone()).ok())
        .unwrap_or_default()
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
/// after the inherited (shell snapshot) environment so both override it.
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

/// Read the enabled variables configured for `provider_id` from the
/// preferences store. Any failure — missing file, unparseable JSON, absent or
/// malformed key — yields no variables, so the agent launches as before.
pub(crate) fn read_agent_env(provider_id: &str) -> Vec<(String, String)> {
    let Some(path) = crate::preferences_store_path_buf() else {
        return Vec::new();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Vec::new();
    };
    enabled_env_pairs(&agent_env_vars_from_preferences(&json, provider_id))
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
