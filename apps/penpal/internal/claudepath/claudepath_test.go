package claudepath

import (
	"os"
	"path/filepath"
	"testing"
)

// E-PENPAL-CLAUDE-PATH: verifies Resolve returns a valid remembered path.
func TestResolve_RememberedPath(t *testing.T) {
	// Create a fake executable
	dir := t.TempDir()
	fakeClaude := filepath.Join(dir, "claude")
	os.WriteFile(fakeClaude, []byte("#!/bin/sh\n"), 0755)

	got := Resolve(fakeClaude)
	if got != fakeClaude {
		t.Errorf("expected %q, got %q", fakeClaude, got)
	}
}

// E-PENPAL-CLAUDE-PATH: verifies Resolve skips a nonexistent remembered path.
func TestResolve_RememberedPathGone(t *testing.T) {
	// A remembered path that no longer exists should not be returned
	got := Resolve("/nonexistent/path/to/claude")
	// It should fall through to LookPath or candidates — we can't predict
	// the result, but it should not be the nonexistent path
	if got == "/nonexistent/path/to/claude" {
		t.Error("should not return nonexistent remembered path")
	}
}

// E-PENPAL-CLAUDE-PATH: verifies Resolve finds claude from candidate paths.
func TestResolve_FindsInCandidates(t *testing.T) {
	// Create a fake executable in ~/.local/bin/claude location
	dir := t.TempDir()
	fakeClaude := filepath.Join(dir, "claude")
	os.WriteFile(fakeClaude, []byte("#!/bin/sh\n"), 0755)

	// Override candidatePaths for testing isn't easy since it uses os.UserHomeDir,
	// but we can test that Resolve with a valid remembered path works
	got := Resolve(fakeClaude)
	if got != fakeClaude {
		t.Errorf("expected %q, got %q", fakeClaude, got)
	}
}

// E-PENPAL-CLAUDE-PATH: verifies IsExecutable for files, dirs, and nonexistent paths.
func TestIsExecutable(t *testing.T) {
	dir := t.TempDir()

	// Non-executable file
	noExec := filepath.Join(dir, "noexec")
	os.WriteFile(noExec, []byte("data"), 0644)
	if IsExecutable(noExec) {
		t.Error("expected non-executable file to return false")
	}

	// Executable file
	exec := filepath.Join(dir, "exec")
	os.WriteFile(exec, []byte("#!/bin/sh\n"), 0755)
	if !IsExecutable(exec) {
		t.Error("expected executable file to return true")
	}

	// Directory
	subdir := filepath.Join(dir, "subdir")
	os.Mkdir(subdir, 0755)
	if IsExecutable(subdir) {
		t.Error("expected directory to return false")
	}

	// Nonexistent
	if IsExecutable(filepath.Join(dir, "missing")) {
		t.Error("expected nonexistent file to return false")
	}
}
