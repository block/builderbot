package watcher

import (
	"os"
	"path/filepath"
	"sort"
	"testing"

	"github.com/loganj/penpal/internal/discovery"
)

func TestWatchProjectRemappedManualSources(t *testing.T) {
	// Create a temporary project directory with a manual source
	projectDir := t.TempDir()
	manualDir := filepath.Join(projectDir, "docs", "api")
	if err := os.MkdirAll(manualDir, 0o755); err != nil {
		t.Fatal(err)
	}

	// Create a worktree directory with the same manual source path
	wtDir := t.TempDir()
	wtManualDir := filepath.Join(wtDir, "docs", "api")
	if err := os.MkdirAll(wtManualDir, 0o755); err != nil {
		t.Fatal(err)
	}
	// Also create the auto-detect dir in the worktree
	wtThoughtsDir := filepath.Join(wtDir, "thoughts")
	if err := os.MkdirAll(wtThoughtsDir, 0o755); err != nil {
		t.Fatal(err)
	}

	project := discovery.Project{
		Name: "proj",
		Path: projectDir,
		Sources: []discovery.FileSource{
			{
				Name:           "thoughts",
				Type:           "tree",
				SourceTypeName: "thoughts",
				RootPath:       filepath.Join(projectDir, "thoughts"),
				Auto:           true,
			},
			{
				Name:           "api-docs",
				Type:           "tree",
				SourceTypeName: "manual",
				RootPath:       manualDir,
				Auto:           false,
			},
		},
		Worktrees: []discovery.Worktree{
			{Name: "main", Path: projectDir, IsMain: true},
			{Name: "feature", Path: wtDir, IsMain: false},
		},
	}

	w, err := New(nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	w.watchProject(project)

	watched := w.watcher.WatchList()
	sort.Strings(watched)

	// The remapped manual source directory in the worktree should be watched
	found := false
	for _, p := range watched {
		if p == wtManualDir {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("expected worktree manual source dir %s to be watched, watched: %v", wtManualDir, watched)
	}

	// The auto-detect dir in the worktree should also be watched
	foundAuto := false
	for _, p := range watched {
		if p == wtThoughtsDir {
			foundAuto = true
			break
		}
	}
	if !foundAuto {
		t.Errorf("expected worktree auto-detect dir %s to be watched, watched: %v", wtThoughtsDir, watched)
	}
}
