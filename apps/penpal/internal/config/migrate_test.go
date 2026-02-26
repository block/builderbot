package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestMigrateProjectDir(t *testing.T) {
	dir := t.TempDir()

	// Create old .birdseye structure
	oldDir := filepath.Join(dir, ".birdseye", "comments")
	if err := os.MkdirAll(oldDir, 0o755); err != nil {
		t.Fatal(err)
	}
	testFile := filepath.Join(oldDir, "foo.json")
	if err := os.WriteFile(testFile, []byte(`{"test":true}`), 0o644); err != nil {
		t.Fatal(err)
	}

	MigrateProjectDir(dir)

	// Verify .penpal exists with contents
	newFile := filepath.Join(dir, ".penpal", "comments", "foo.json")
	if _, err := os.Stat(newFile); err != nil {
		t.Fatalf("expected migrated file at %s: %v", newFile, err)
	}

	// Verify .birdseye is gone
	if _, err := os.Stat(filepath.Join(dir, ".birdseye")); !os.IsNotExist(err) {
		t.Fatal("expected .birdseye to be removed after migration")
	}
}

func TestMigrateProjectDir_NoOp(t *testing.T) {
	dir := t.TempDir()

	// No .birdseye directory — should be a no-op
	MigrateProjectDir(dir)

	if _, err := os.Stat(filepath.Join(dir, ".penpal")); !os.IsNotExist(err) {
		t.Fatal("expected no .penpal directory when there's nothing to migrate")
	}
}

func TestMigrateProjectDir_NoClobber(t *testing.T) {
	dir := t.TempDir()

	// Create both old and new directories
	os.MkdirAll(filepath.Join(dir, ".birdseye"), 0o755)
	os.MkdirAll(filepath.Join(dir, ".penpal"), 0o755)

	// Write a marker file in .penpal
	marker := filepath.Join(dir, ".penpal", "marker.txt")
	os.WriteFile(marker, []byte("keep"), 0o644)

	MigrateProjectDir(dir)

	// .penpal should still have our marker (not clobbered)
	data, err := os.ReadFile(marker)
	if err != nil || string(data) != "keep" {
		t.Fatal("expected .penpal to not be clobbered")
	}
}
