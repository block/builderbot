//! Custom theme discovery and loading.
//!
//! Discovers VS Code theme JSON files in ~/.config/staged/themes/
//! and provides them to the frontend for use with Shiki.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Metadata about a custom theme (returned to frontend).
#[derive(Debug, Clone, Serialize)]
pub struct CustomTheme {
    /// Theme name (from JSON or filename)
    pub name: String,
    /// Whether this is a light theme
    pub is_light: bool,
    /// Full path to the theme file
    pub path: String,
}

/// Minimal VS Code theme structure for parsing metadata.
#[derive(Debug, Deserialize)]
struct VsCodeTheme {
    name: Option<String>,
    #[serde(rename = "type")]
    theme_type: Option<String>,
}

/// Get the custom themes directory path.
fn themes_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("staged").join("themes"))
}

/// Discover all custom themes in the themes directory.
pub fn discover_custom_themes() -> Vec<CustomTheme> {
    let Some(dir) = themes_dir() else {
        return vec![];
    };

    if !dir.exists() {
        return vec![];
    }

    let Ok(entries) = fs::read_dir(&dir) else {
        return vec![];
    };

    let mut themes = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();

        // Only process .json files
        if path.extension().is_some_and(|ext| ext == "json") {
            if let Some(theme) = load_theme_metadata(&path) {
                themes.push(theme);
            }
        }
    }

    // Sort alphabetically by name
    themes.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    themes
}

/// Load metadata from a theme file.
fn load_theme_metadata(path: &PathBuf) -> Option<CustomTheme> {
    let content = fs::read_to_string(path).ok()?;
    let parsed: VsCodeTheme = serde_json::from_str(&content).ok()?;

    // Get name from JSON or fall back to filename
    let name = parsed.name.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    // Determine if light theme (default to dark)
    let is_light = parsed
        .theme_type
        .as_ref()
        .is_some_and(|t| t.to_lowercase() == "light");

    Some(CustomTheme {
        name,
        is_light,
        path: path.to_string_lossy().to_string(),
    })
}

/// Read the full theme JSON content for loading into Shiki.
pub fn read_theme_file(path: &str) -> Result<String, String> {
    // Security: ensure the path is within the themes directory
    let themes_dir = themes_dir().ok_or("Cannot determine config directory")?;
    let requested = PathBuf::from(path);

    // Canonicalize both paths to prevent directory traversal
    let canonical_themes = themes_dir
        .canonicalize()
        .map_err(|_| "Themes directory does not exist")?;
    let canonical_requested = requested
        .canonicalize()
        .map_err(|e| format!("Cannot access theme file: {e}"))?;

    if !canonical_requested.starts_with(&canonical_themes) {
        return Err("Access denied: path outside themes directory".to_string());
    }

    fs::read_to_string(&canonical_requested).map_err(|e| format!("Cannot read theme: {e}"))
}

/// Ensure the themes directory exists.
pub fn ensure_themes_dir() -> Result<PathBuf, String> {
    let dir = themes_dir().ok_or("Cannot determine config directory")?;
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create themes directory: {e}"))?;
    Ok(dir)
}

/// Result of validating a theme file.
#[derive(Debug, Clone, Serialize)]
pub struct ThemeValidation {
    /// Whether the theme is valid
    pub valid: bool,
    /// Theme name (if valid)
    pub name: Option<String>,
    /// Whether it's a light theme (if valid)
    pub is_light: Option<bool>,
    /// Error message (if invalid)
    pub error: Option<String>,
}

/// Validate theme JSON content without installing.
pub fn validate_theme(content: &str) -> ThemeValidation {
    // Try to parse as JSON first
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(content);
    let Ok(json) = parsed else {
        return ThemeValidation {
            valid: false,
            name: None,
            is_light: None,
            error: Some("Invalid JSON".to_string()),
        };
    };

    // Check for required VS Code theme structure
    // At minimum, we need either "colors" or "tokenColors"
    let has_colors = json.get("colors").is_some();
    let has_token_colors = json.get("tokenColors").is_some();

    if !has_colors && !has_token_colors {
        return ThemeValidation {
            valid: false,
            name: None,
            is_light: None,
            error: Some("Not a valid VS Code theme: missing 'colors' or 'tokenColors'".to_string()),
        };
    }

    // Extract metadata
    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let is_light = json
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t.to_lowercase() == "light")
        .unwrap_or(false);

    ThemeValidation {
        valid: true,
        name,
        is_light: Some(is_light),
        error: None,
    }
}

/// Install a theme by copying content to the themes directory.
/// Returns the installed theme metadata.
pub fn install_theme(content: &str, filename: &str) -> Result<CustomTheme, String> {
    // Validate first
    let validation = validate_theme(content);
    if !validation.valid {
        return Err(validation.error.unwrap_or("Invalid theme".to_string()));
    }

    // Ensure themes directory exists
    let dir = ensure_themes_dir()?;

    // Sanitize filename - only allow alphanumeric, dash, underscore
    let safe_name: String = filename
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Ensure .json extension
    let final_name = if safe_name.to_lowercase().ends_with(".json") {
        safe_name
    } else {
        format!("{safe_name}.json")
    };

    let dest_path = dir.join(&final_name);

    // Write the file
    fs::write(&dest_path, content).map_err(|e| format!("Failed to write theme: {e}"))?;

    // Load and return the metadata
    load_theme_metadata(&dest_path).ok_or_else(|| "Failed to load installed theme".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_themes_dir() {
        // Just verify it returns something reasonable
        let dir = themes_dir();
        assert!(dir.is_some());
        let path = dir.unwrap();
        assert!(path.ends_with("staged/themes"));
    }
}
