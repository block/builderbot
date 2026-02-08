package config

import (
	"encoding/json"
	"os"
	"path/filepath"
)

// Config is the persistent birdseye configuration.
type Config struct {
	Workspaces []Workspace     `json:"workspaces"`
	Projects   []ProjectConfig `json:"projects,omitempty"`
}

// Workspace is a directory that is scanned for projects.
type Workspace struct {
	Path string `json:"path"`
	Name string `json:"name,omitempty"` // display name, defaults to basename
}

// ProjectConfig is a standalone project opened explicitly.
type ProjectConfig struct {
	Path    string         `json:"path"`
	Name    string         `json:"name,omitempty"`    // display name, defaults to basename
	Sources []SourceConfig `json:"sources,omitempty"` // user-added sources (thoughts/ is implicit)
}

// SourceConfig describes a user-added file source within a project.
type SourceConfig struct {
	Type  string   `json:"type"`            // "tree" or "files"
	Path  string   `json:"path,omitempty"`  // relative to project root, for type="tree"
	Files []string `json:"files,omitempty"` // relative paths, for type="files"
	Name  string   `json:"name,omitempty"`  // display name
}

// DefaultConfigPath returns the default config file location.
func DefaultConfigPath() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".config", "birdseye", "config.json")
}

// Load reads the config from the given path.
// Returns an empty config if the file doesn't exist.
func Load(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return &Config{}, nil
		}
		return nil, err
	}

	var cfg Config
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, err
	}
	return &cfg, nil
}

// Save writes the config to the given path atomically.
func Save(path string, cfg *Config) error {
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}

	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}
	data = append(data, '\n')

	// Atomic write: write to temp file then rename
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// EnsureDefaults fills in default workspace if none are configured.
// If rootOverride is non-empty, it is used instead of ~/Development.
func EnsureDefaults(cfg *Config, rootOverride string) {
	if len(cfg.Workspaces) > 0 || len(cfg.Projects) > 0 {
		return
	}

	defaultRoot := rootOverride
	if defaultRoot == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return
		}
		defaultRoot = filepath.Join(home, "Development")
	}

	cfg.Workspaces = []Workspace{{Path: defaultRoot}}
}
