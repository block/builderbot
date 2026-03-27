package config

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// Config is the persistent penpal configuration.
type Config struct {
	Workspaces     []Workspace               `json:"workspaces"`
	Projects       []ProjectConfig           `json:"projects,omitempty"`
	ProjectSources map[string][]SourceConfig `json:"projectSources,omitempty"` // key: absolute project path, for workspace projects
	ClaudePath     string                    `json:"claudePath,omitempty"`     // remembered absolute path to claude binary
}

// Workspace is a directory that is scanned for projects.
type Workspace struct {
	Path string `json:"path"`
	Name string `json:"name,omitempty"` // display name, defaults to basename
}

// DisplayName returns the workspace's display name, defaulting to the basename of its path.
func (w Workspace) DisplayName() string {
	if w.Name != "" {
		return w.Name
	}
	return filepath.Base(w.Path)
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
// Set PENPAL_CONFIG to override (used by e2e tests for isolation).
// E-PENPAL-CONFIG: resolves ~/.config/penpal/config.json path.
func DefaultConfigPath() string {
	if p := os.Getenv("PENPAL_CONFIG"); p != "" {
		return p
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".config", "penpal", "config.json")
}

// Load reads the config from the given path.
// Returns an empty config if the file doesn't exist.
// E-PENPAL-CONFIG: loads config from ~/.config/penpal/config.json.
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
// E-PENPAL-CONFIG: atomic write via .tmp + rename.
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

// PortFilePath returns the path to the server port file.
// E-PENPAL-PORT-FILE: resolves ~/.config/penpal/server.port path.
func PortFilePath() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".config", "penpal", "server.port")
}

// WritePortFile writes the server port to the port file.
// E-PENPAL-PORT-FILE: writes server.port on startup.
func WritePortFile(port int) error {
	path := PortFilePath()
	if path == "" {
		return fmt.Errorf("cannot determine port file path")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}
	return os.WriteFile(path, []byte(strconv.Itoa(port)), 0644)
}

// ReadPortFile reads the server port from the port file.
// Returns 0 if the file does not exist or cannot be read.
// E-PENPAL-PORT-FILE: reads server.port for CLI discovery.
func ReadPortFile() int {
	path := PortFilePath()
	if path == "" {
		return 0
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return 0
	}
	port, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		return 0
	}
	return port
}

// RemovePortFile removes the server port file.
// E-PENPAL-PORT-FILE: removes server.port on shutdown.
func RemovePortFile() {
	path := PortFilePath()
	if path != "" {
		os.Remove(path)
	}
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
