package publish

import (
	"os"
	"testing"
	"time"
)

func TestLoadSaveState(t *testing.T) {
	dir := t.TempDir()

	// Loading from empty directory returns empty map
	state, err := LoadState(dir)
	if err != nil {
		t.Fatalf("LoadState failed: %v", err)
	}
	if len(state) != 0 {
		t.Errorf("expected empty state, got %d entries", len(state))
	}

	// Save a state entry
	now := time.Now().Truncate(time.Second)
	err = SaveState(dir, "docs/readme.md", &PublishState{
		SiteName:      "penpal-test-readme",
		URL:           "https://blockcell.sqprod.co/sites/penpal-test-readme/",
		LastPublished: now,
	})
	if err != nil {
		t.Fatalf("SaveState failed: %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(dir + "/.penpal/publish.json"); err != nil {
		t.Fatalf("publish.json not created: %v", err)
	}

	// Load and verify
	state, err = LoadState(dir)
	if err != nil {
		t.Fatalf("LoadState failed: %v", err)
	}
	if len(state) != 1 {
		t.Fatalf("expected 1 entry, got %d", len(state))
	}
	entry := state["docs/readme.md"]
	if entry == nil {
		t.Fatal("missing entry for docs/readme.md")
	}
	if entry.SiteName != "penpal-test-readme" {
		t.Errorf("unexpected site name: %s", entry.SiteName)
	}
	if entry.URL != "https://blockcell.sqprod.co/sites/penpal-test-readme/" {
		t.Errorf("unexpected URL: %s", entry.URL)
	}

	// Save a second entry, verify both exist
	err = SaveState(dir, "plans/plan.md", &PublishState{
		SiteName:      "penpal-test-plan",
		URL:           "https://blockcell.sqprod.co/sites/penpal-test-plan/",
		LastPublished: now,
	})
	if err != nil {
		t.Fatalf("SaveState (2nd) failed: %v", err)
	}

	state, err = LoadState(dir)
	if err != nil {
		t.Fatalf("LoadState failed: %v", err)
	}
	if len(state) != 2 {
		t.Fatalf("expected 2 entries, got %d", len(state))
	}
}
