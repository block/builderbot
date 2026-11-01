//! Display titles for sessions that haven't produced their artifact yet.
//!
//! The branch timeline and the child-session badges under a project session
//! both show in-flight sessions, and they must name them the same way: these
//! helpers are the single statement of each rule.

/// The agent-pushed ACP session title, when set and non-empty.
///
/// Applied at read time only, as an interim display name for in-progress
/// pending commits and untitled reviews. Notes always use their stored titles.
pub fn non_empty_acp_title(acp_title: Option<&str>) -> Option<String> {
    acp_title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// The subject shown for a commit whose session hasn't produced a sha yet:
/// the live ACP title, else the session's prompt, else a static placeholder.
pub fn pending_commit_subject(acp_title: Option<&str>, prompt: Option<&str>) -> String {
    non_empty_acp_title(acp_title)
        .or_else(|| prompt.map(str::to_string))
        .unwrap_or_else(|| "Pending commit".to_string())
}

/// A review's displayed title. A stored title always wins; while the review's
/// session is still running and the title is unset, the ACP title stands in,
/// so an untitled running review isn't a static placeholder.
pub fn review_title(
    stored: Option<String>,
    session_status: Option<&str>,
    acp_title: Option<&str>,
) -> Option<String> {
    match stored {
        None if session_status == Some("running") => non_empty_acp_title(acp_title),
        title => title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_title_is_trimmed_and_blank_is_none() {
        assert_eq!(
            non_empty_acp_title(Some("  Fix it  ")).as_deref(),
            Some("Fix it")
        );
        assert_eq!(non_empty_acp_title(Some("   ")), None);
        assert_eq!(non_empty_acp_title(None), None);
    }

    #[test]
    fn pending_commit_prefers_acp_title_then_prompt_then_placeholder() {
        assert_eq!(
            pending_commit_subject(Some("Live title"), Some("prompt")),
            "Live title"
        );
        assert_eq!(pending_commit_subject(Some(" "), Some("prompt")), "prompt");
        assert_eq!(pending_commit_subject(None, None), "Pending commit");
    }

    #[test]
    fn review_title_uses_acp_title_only_while_running_and_untitled() {
        assert_eq!(
            review_title(Some("Stored".into()), Some("running"), Some("Live")).as_deref(),
            Some("Stored")
        );
        assert_eq!(
            review_title(None, Some("running"), Some("Live")).as_deref(),
            Some("Live")
        );
        assert_eq!(review_title(None, Some("queued"), Some("Live")), None);
        assert_eq!(review_title(None, Some("running"), None), None);
    }
}
