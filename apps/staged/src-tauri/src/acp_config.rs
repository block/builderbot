//! Normalization helpers for ACP session configuration options.

use acp_client::AcpSessionConfigOptionSelection;
use agent_client_protocol::schema::v1::{
    SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
    SessionConfigSelectOptions,
};
use serde::{Deserialize, Serialize};

use crate::store::{AcpConfigSelection, AcpConfigValueSelection};

/// Product-facing ACP configuration selectors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NormalizedAcpConfigOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<NormalizedAcpConfigSelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) effort: Option<NormalizedAcpConfigSelector>,
}

/// A normalized select-style ACP configuration option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NormalizedAcpConfigSelector {
    pub(crate) config_id: String,
    pub(crate) label: String,
    pub(crate) current_value_id: String,
    pub(crate) options: Vec<NormalizedAcpConfigValueOption>,
}

/// One flattened selectable value for an ACP configuration option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NormalizedAcpConfigValueOption {
    pub(crate) value_id: String,
    pub(crate) label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) group_label: Option<String>,
}

/// Extract the model and reasoning-effort selectors from ACP config options.
pub(crate) fn normalize_acp_config_options(
    config_options: &[SessionConfigOption],
) -> NormalizedAcpConfigOptions {
    NormalizedAcpConfigOptions {
        model: normalize_selector_for_category(config_options, &SessionConfigOptionCategory::Model),
        effort: normalize_selector_for_category(
            config_options,
            &SessionConfigOptionCategory::ThoughtLevel,
        ),
    }
}

pub(crate) fn selected_acp_config_options(
    selection: Option<&AcpConfigSelection>,
) -> Vec<AcpSessionConfigOptionSelection> {
    let Some(selection) = selection else {
        return Vec::new();
    };

    let mut options = Vec::new();
    if let Some(model) = &selection.model {
        options.push(selected_config_option(
            SessionConfigOptionCategory::Model,
            model,
        ));
    }
    if let Some(effort) = &selection.effort {
        options.push(selected_config_option(
            SessionConfigOptionCategory::ThoughtLevel,
            effort,
        ));
    }
    options
}

fn selected_config_option(
    category: SessionConfigOptionCategory,
    selection: &AcpConfigValueSelection,
) -> AcpSessionConfigOptionSelection {
    AcpSessionConfigOptionSelection {
        category,
        config_id: selection.config_id.clone(),
        value_id: selection.value_id.clone(),
    }
}

/// Preferences-store key holding the diagram sub-session override (General
/// settings → Diagram generation). Written by the frontend preferences store
/// and read directly from `preferences.json` here, mirroring `branch-prefix`.
const DIAGRAM_SUBSESSION_CONFIG_KEY: &str = "diagram-subsession-config";

/// The agent/model/effort the `generate_pikchr` diagram sub-session runs under,
/// distinct from the session that invoked the tool. Every field is optional: an
/// unset (or empty) provider falls the sub-session back to the invoking
/// session's agent, and unset model/effort fall back to that agent's defaults —
/// so an all-unset config reproduces the pre-setting behaviour. Model/effort
/// value ids are meaningful only relative to the provider they were chosen for,
/// so they are applied only when a provider is configured (see
/// [`Self::config_options`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagramSubsessionConfig {
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<AcpConfigValueSelection>,
    #[serde(default)]
    pub(crate) effort: Option<AcpConfigValueSelection>,
}

impl DiagramSubsessionConfig {
    /// The configured provider id to run the sub-session under, if any. Blank
    /// (whitespace-only) values read as unset.
    pub(crate) fn provider_id(&self) -> Option<&str> {
        self.provider
            .as_deref()
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
    }

    /// The model/effort selections as ACP config options to apply per turn.
    ///
    /// Returns empty when no provider is configured: the stored model/effort
    /// value ids belong to the configured provider, so applying them against a
    /// different (inherited) agent would be meaningless.
    pub(crate) fn config_options(&self) -> Vec<AcpSessionConfigOptionSelection> {
        if self.provider_id().is_none() {
            return Vec::new();
        }
        selected_acp_config_options(Some(&AcpConfigSelection {
            model: self.model.clone(),
            effort: self.effort.clone(),
        }))
    }
}

/// Read the diagram sub-session override from the preferences store. Any
/// failure — missing file, unparseable JSON, absent or malformed key — yields
/// the default (all-unset) config, so the sub-session simply inherits the
/// invoking session's agent at its default model/effort.
pub(crate) fn read_diagram_subsession_config() -> DiagramSubsessionConfig {
    let Some(path) = crate::preferences_store_path_buf() else {
        return DiagramSubsessionConfig::default();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return DiagramSubsessionConfig::default();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return DiagramSubsessionConfig::default();
    };
    json.get(DIAGRAM_SUBSESSION_CONFIG_KEY)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

fn normalize_selector_for_category(
    config_options: &[SessionConfigOption],
    category: &SessionConfigOptionCategory,
) -> Option<NormalizedAcpConfigSelector> {
    config_options
        .iter()
        .filter(|option| option.category.as_ref() == Some(category))
        .find_map(normalize_select_option)
}

fn normalize_select_option(
    config_option: &SessionConfigOption,
) -> Option<NormalizedAcpConfigSelector> {
    let SessionConfigKind::Select(select) = &config_option.kind else {
        return None;
    };

    Some(NormalizedAcpConfigSelector {
        config_id: config_option.id.to_string(),
        label: config_option.name.clone(),
        current_value_id: select.current_value.to_string(),
        options: flatten_select_options(&select.options),
    })
}

fn flatten_select_options(
    options: &SessionConfigSelectOptions,
) -> Vec<NormalizedAcpConfigValueOption> {
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|option| normalize_value(option, None))
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| {
                group
                    .options
                    .iter()
                    .map(|option| normalize_value(option, Some(&group.name)))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_value(
    option: &SessionConfigSelectOption,
    group_label: Option<&str>,
) -> NormalizedAcpConfigValueOption {
    NormalizedAcpConfigValueOption {
        value_id: option.value.to_string(),
        label: option.name.clone(),
        group_label: group_label.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        SessionConfigBoolean, SessionConfigSelectGroup, SessionConfigSelectOption,
    };

    #[test]
    fn extracts_model_and_effort_selectors() {
        let options = vec![
            SessionConfigOption::select(
                "model",
                "Model",
                "gpt-5",
                vec![
                    SessionConfigSelectOption::new("gpt-5", "GPT-5"),
                    SessionConfigSelectOption::new("gpt-5-mini", "GPT-5 mini"),
                ],
            )
            .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "reasoning",
                "Reasoning",
                "high",
                vec![
                    SessionConfigSelectOption::new("low", "Low"),
                    SessionConfigSelectOption::new("high", "High"),
                ],
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ];

        let normalized = normalize_acp_config_options(&options);

        let model = normalized.model.expect("model selector");
        assert_eq!(model.config_id, "model");
        assert_eq!(model.current_value_id, "gpt-5");
        assert_eq!(
            model.options,
            vec![
                NormalizedAcpConfigValueOption {
                    value_id: "gpt-5".to_string(),
                    label: "GPT-5".to_string(),
                    group_label: None,
                },
                NormalizedAcpConfigValueOption {
                    value_id: "gpt-5-mini".to_string(),
                    label: "GPT-5 mini".to_string(),
                    group_label: None,
                },
            ]
        );

        let effort = normalized.effort.expect("effort selector");
        assert_eq!(effort.config_id, "reasoning");
        assert_eq!(effort.current_value_id, "high");
        assert_eq!(effort.options[1].value_id, "high");
    }

    #[test]
    fn filters_unsupported_options() {
        let options = vec![
            SessionConfigOption::select(
                "mode",
                "Mode",
                "default",
                vec![SessionConfigSelectOption::new("default", "Default")],
            )
            .category(SessionConfigOptionCategory::Mode),
            SessionConfigOption::new(
                "model_toggle",
                "Model toggle",
                SessionConfigKind::Boolean(SessionConfigBoolean::new(false)),
            )
            .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "model",
                "Model",
                "opus",
                vec![SessionConfigSelectOption::new("opus", "Opus")],
            )
            .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "effort",
                "Effort",
                "medium",
                vec![SessionConfigSelectOption::new("medium", "Medium")],
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ];

        let normalized = normalize_acp_config_options(&options);

        assert_eq!(
            normalized.model.expect("model selector").current_value_id,
            "opus"
        );
        assert!(normalized.effort.is_some());
    }

    #[test]
    fn flattens_grouped_options() {
        let options = vec![SessionConfigOption::select(
            "model",
            "Model",
            "sonnet",
            vec![
                SessionConfigSelectGroup::new(
                    "fast",
                    "Fast",
                    vec![SessionConfigSelectOption::new("haiku", "Haiku")],
                ),
                SessionConfigSelectGroup::new(
                    "smart",
                    "Smart",
                    vec![
                        SessionConfigSelectOption::new("sonnet", "Sonnet"),
                        SessionConfigSelectOption::new("opus", "Opus"),
                    ],
                ),
            ],
        )
        .category(SessionConfigOptionCategory::Model)];

        let normalized = normalize_acp_config_options(&options);
        let model = normalized.model.expect("model selector");

        assert_eq!(model.current_value_id, "sonnet");
        assert_eq!(
            model.options,
            vec![
                NormalizedAcpConfigValueOption {
                    value_id: "haiku".to_string(),
                    label: "Haiku".to_string(),
                    group_label: Some("Fast".to_string()),
                },
                NormalizedAcpConfigValueOption {
                    value_id: "sonnet".to_string(),
                    label: "Sonnet".to_string(),
                    group_label: Some("Smart".to_string()),
                },
                NormalizedAcpConfigValueOption {
                    value_id: "opus".to_string(),
                    label: "Opus".to_string(),
                    group_label: Some("Smart".to_string()),
                },
            ]
        );
    }

    #[test]
    fn returns_none_for_missing_categories() {
        let options = vec![
            SessionConfigOption::select(
                "uncategorized_model",
                "Model",
                "default",
                vec![SessionConfigSelectOption::new("default", "Default")],
            ),
            SessionConfigOption::select(
                "custom",
                "Custom",
                "custom",
                vec![SessionConfigSelectOption::new("custom", "Custom")],
            )
            .category(SessionConfigOptionCategory::Other("_custom".to_string())),
        ];

        let normalized = normalize_acp_config_options(&options);

        assert!(normalized.model.is_none());
        assert!(normalized.effort.is_none());
    }

    #[test]
    fn selected_config_options_preserve_model_then_effort_order() {
        let selection = AcpConfigSelection {
            model: Some(AcpConfigValueSelection {
                config_id: "model".to_string(),
                value_id: "sonnet".to_string(),
                label: Some("Sonnet".to_string()),
            }),
            effort: Some(AcpConfigValueSelection {
                config_id: "reasoning".to_string(),
                value_id: "high".to_string(),
                label: Some("High".to_string()),
            }),
        };

        let selected = selected_acp_config_options(Some(&selection));

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].category, SessionConfigOptionCategory::Model);
        assert_eq!(selected[0].config_id, "model");
        assert_eq!(selected[0].value_id, "sonnet");
        assert_eq!(
            selected[1].category,
            SessionConfigOptionCategory::ThoughtLevel
        );
        assert_eq!(selected[1].config_id, "reasoning");
        assert_eq!(selected[1].value_id, "high");
    }

    #[test]
    fn diagram_config_deserializes_provider_model_and_effort() {
        let config: DiagramSubsessionConfig = serde_json::from_value(serde_json::json!({
            "provider": "claude",
            "model": { "configId": "model", "valueId": "opus", "label": "Opus" },
            "effort": { "configId": "reasoning", "valueId": "high", "label": "High" },
        }))
        .expect("valid diagram config");

        assert_eq!(config.provider_id(), Some("claude"));

        let options = config.config_options();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].category, SessionConfigOptionCategory::Model);
        assert_eq!(options[0].value_id, "opus");
        assert_eq!(
            options[1].category,
            SessionConfigOptionCategory::ThoughtLevel
        );
        assert_eq!(options[1].value_id, "high");
    }

    #[test]
    fn diagram_config_treats_blank_provider_as_unset() {
        let config = DiagramSubsessionConfig {
            provider: Some("   ".to_string()),
            model: Some(AcpConfigValueSelection {
                config_id: "model".to_string(),
                value_id: "opus".to_string(),
                label: None,
            }),
            effort: None,
        };

        assert_eq!(config.provider_id(), None);
    }

    #[test]
    fn diagram_config_without_provider_applies_no_config_options() {
        // Model/effort value ids are provider-specific; with no configured
        // provider the sub-session inherits the invoking agent and must not
        // apply overrides meant for a different one.
        let config = DiagramSubsessionConfig {
            provider: None,
            model: Some(AcpConfigValueSelection {
                config_id: "model".to_string(),
                value_id: "opus".to_string(),
                label: None,
            }),
            effort: Some(AcpConfigValueSelection {
                config_id: "reasoning".to_string(),
                value_id: "high".to_string(),
                label: None,
            }),
        };

        assert!(config.config_options().is_empty());
    }

    #[test]
    fn diagram_config_defaults_when_missing() {
        let config: DiagramSubsessionConfig =
            serde_json::from_value(serde_json::json!({})).expect("empty object is valid");
        assert_eq!(config, DiagramSubsessionConfig::default());
        assert_eq!(config.provider_id(), None);
        assert!(config.config_options().is_empty());
    }
}
