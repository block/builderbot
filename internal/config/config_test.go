package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestLoadNonexistent(t *testing.T) {
	cfg, err := Load("/nonexistent/path/config.json")
	if err != nil {
		t.Fatalf("expected no error for missing file, got: %v", err)
	}
	if len(cfg.Workspaces) != 0 {
		t.Errorf("expected empty workspaces, got %d", len(cfg.Workspaces))
	}
	if len(cfg.Projects) != 0 {
		t.Errorf("expected empty projects, got %d", len(cfg.Projects))
	}
}

func TestSaveAndLoad(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "subdir", "config.json")

	cfg := &Config{
		Workspaces: []Workspace{{Path: "/home/user/dev", Name: "Dev"}},
		Projects: []ProjectConfig{{
			Path: "/tmp/standalone",
			Sources: []SourceConfig{{
				Type: "tree",
				Path: "docs",
				Name: "Documentation",
			}},
		}},
	}

	if err := Save(path, cfg); err != nil {
		t.Fatalf("Save: %v", err)
	}

	loaded, err := Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}

	if len(loaded.Workspaces) != 1 {
		t.Fatalf("expected 1 workspace, got %d", len(loaded.Workspaces))
	}
	if loaded.Workspaces[0].Path != "/home/user/dev" {
		t.Errorf("unexpected workspace path: %s", loaded.Workspaces[0].Path)
	}
	if loaded.Workspaces[0].Name != "Dev" {
		t.Errorf("unexpected workspace name: %s", loaded.Workspaces[0].Name)
	}
	if len(loaded.Projects) != 1 {
		t.Fatalf("expected 1 project, got %d", len(loaded.Projects))
	}
	if loaded.Projects[0].Path != "/tmp/standalone" {
		t.Errorf("unexpected project path: %s", loaded.Projects[0].Path)
	}
	if len(loaded.Projects[0].Sources) != 1 {
		t.Fatalf("expected 1 source, got %d", len(loaded.Projects[0].Sources))
	}
	if loaded.Projects[0].Sources[0].Type != "tree" {
		t.Errorf("unexpected source type: %s", loaded.Projects[0].Sources[0].Type)
	}
}

func TestEnsureDefaults_Empty(t *testing.T) {
	cfg := &Config{}
	EnsureDefaults(cfg, "")

	if len(cfg.Workspaces) != 1 {
		t.Fatalf("expected 1 default workspace, got %d", len(cfg.Workspaces))
	}

	home, _ := os.UserHomeDir()
	expected := filepath.Join(home, "Development")
	if cfg.Workspaces[0].Path != expected {
		t.Errorf("expected default workspace path %s, got %s", expected, cfg.Workspaces[0].Path)
	}
}

func TestEnsureDefaults_WithOverride(t *testing.T) {
	cfg := &Config{}
	EnsureDefaults(cfg, "/custom/root")

	if len(cfg.Workspaces) != 1 {
		t.Fatalf("expected 1 workspace, got %d", len(cfg.Workspaces))
	}
	if cfg.Workspaces[0].Path != "/custom/root" {
		t.Errorf("expected /custom/root, got %s", cfg.Workspaces[0].Path)
	}
}

func TestEnsureDefaults_AlreadyConfigured(t *testing.T) {
	cfg := &Config{
		Workspaces: []Workspace{{Path: "/existing"}},
	}
	EnsureDefaults(cfg, "/override")

	if len(cfg.Workspaces) != 1 {
		t.Fatalf("expected 1 workspace, got %d", len(cfg.Workspaces))
	}
	if cfg.Workspaces[0].Path != "/existing" {
		t.Errorf("expected /existing preserved, got %s", cfg.Workspaces[0].Path)
	}
}

func TestSaveAtomic(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.json")

	cfg := &Config{Workspaces: []Workspace{{Path: "/test"}}}
	if err := Save(path, cfg); err != nil {
		t.Fatalf("Save: %v", err)
	}

	// Verify no temp file remains
	if _, err := os.Stat(path + ".tmp"); !os.IsNotExist(err) {
		t.Error("temp file should not exist after save")
	}

	// Verify file is valid JSON
	if _, err := Load(path); err != nil {
		t.Errorf("saved file should be loadable: %v", err)
	}
}
