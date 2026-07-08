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
}
