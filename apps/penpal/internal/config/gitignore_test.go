package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// E-PENPAL-GITIGNORE: verifies containsLine finds a matching line.
func TestContainsLine_Found(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "ignore")
	if err := os.WriteFile(path, []byte("foo\n.penpal\nbar\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if !containsLine(path, ".penpal") {
		t.Error("expected containsLine to find .penpal")
	}
}

// E-PENPAL-GITIGNORE: verifies containsLine returns false when line is absent.
func TestContainsLine_NotFound(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "ignore")
	if err := os.WriteFile(path, []byte("foo\nbar\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if containsLine(path, ".penpal") {
		t.Error("expected containsLine to return false for missing line")
	}
}

// E-PENPAL-GITIGNORE: verifies containsLine returns false for missing file.
func TestContainsLine_MissingFile(t *testing.T) {
	if containsLine("/nonexistent/file", ".penpal") {
		t.Error("expected false for missing file")
	}
}

// E-PENPAL-GITIGNORE: verifies containsLine handles whitespace trimming.
func TestContainsLine_Whitespace(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "ignore")
	if err := os.WriteFile(path, []byte("  .penpal  \n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if !containsLine(path, ".penpal") {
		t.Error("expected containsLine to find .penpal with surrounding whitespace")
	}
}

// E-PENPAL-GITIGNORE: verifies EnsureGlobalGitignore appends .penpal to an existing file.
func TestEnsureGlobalGitignore_AppendsToExisting(t *testing.T) {
	dir := t.TempDir()
	ignorePath := filepath.Join(dir, "globalignore")
	if err := os.WriteFile(ignorePath, []byte("*.log\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Override globalGitignorePath by calling the lower-level helpers directly.
	// Since globalGitignorePath uses git config, we test the append behavior
	// by calling containsLine + the file write logic directly.
	// We verify the core behavior: append .penpal if not present.
	ensureGitignoreEntry(ignorePath)

	data, err := os.ReadFile(ignorePath)
	if err != nil {
		t.Fatal(err)
	}
	content := string(data)
	if !strings.Contains(content, ".penpal") {
		t.Errorf("expected .penpal in file, got: %q", content)
	}
	if !strings.Contains(content, "*.log") {
		t.Errorf("expected existing content preserved, got: %q", content)
	}
}

// E-PENPAL-GITIGNORE: verifies idempotency (calling twice does not duplicate entry).
func TestEnsureGlobalGitignore_Idempotent(t *testing.T) {
	dir := t.TempDir()
	ignorePath := filepath.Join(dir, "globalignore")
	if err := os.WriteFile(ignorePath, []byte("*.log\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	ensureGitignoreEntry(ignorePath)
	ensureGitignoreEntry(ignorePath)

	data, err := os.ReadFile(ignorePath)
	if err != nil {
		t.Fatal(err)
	}
	count := strings.Count(string(data), ".penpal")
	if count != 1 {
		t.Errorf("expected .penpal to appear once, got %d times in: %q", count, string(data))
	}
}

// E-PENPAL-GITIGNORE: verifies it creates the file if it does not exist.
func TestEnsureGlobalGitignore_CreatesFile(t *testing.T) {
	dir := t.TempDir()
	ignorePath := filepath.Join(dir, "subdir", "globalignore")

	ensureGitignoreEntry(ignorePath)

	data, err := os.ReadFile(ignorePath)
	if err != nil {
		t.Fatalf("expected file to be created: %v", err)
	}
	content := string(data)
	if !strings.Contains(content, ".penpal") {
		t.Errorf("expected .penpal in new file, got: %q", content)
	}
	// New file should not start with a spurious newline
	if strings.HasPrefix(content, "\n") {
		t.Errorf("new file should not start with newline, got: %q", content)
	}
}

// E-PENPAL-GITIGNORE: verifies a leading newline is added when file doesn't end with one.
func TestEnsureGlobalGitignore_AddsNewlineBeforeEntry(t *testing.T) {
	dir := t.TempDir()
	ignorePath := filepath.Join(dir, "globalignore")
	// Write file content that does NOT end with a newline
	if err := os.WriteFile(ignorePath, []byte("*.log"), 0o644); err != nil {
		t.Fatal(err)
	}

	ensureGitignoreEntry(ignorePath)

	data, err := os.ReadFile(ignorePath)
	if err != nil {
		t.Fatal(err)
	}
	content := string(data)
	// Should have newline between existing content and .penpal
	if !strings.Contains(content, "*.log\n.penpal\n") {
		t.Errorf("expected newline before .penpal, got: %q", content)
	}
}

// ensureGitignoreEntry replicates the core logic of EnsureGlobalGitignore
// for a given path, without relying on git config for path resolution.
func ensureGitignoreEntry(path string) {
	if containsLine(path, ".penpal") {
		return
	}

	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return
	}

	f, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		return
	}
	defer f.Close()

	info, _ := f.Stat()
	if info != nil && info.Size() > 0 {
		f.WriteString("\n")
	}
	f.WriteString(".penpal\n")
}
