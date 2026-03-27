package watcher

import (
	"os"
	"path/filepath"
	"sort"
	"testing"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/discovery"
)

// E-PENPAL-WATCHER: verifies FocusProject watches worktree manual sources and auto-detect dirs.
func TestFocusProjectRemappedManualSources(t *testing.T) {
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

	c := cache.New()
	c.SetProjects([]discovery.Project{project})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	w.FocusProject(project.QualifiedName())

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

// E-PENPAL-WATCHER: verifies switching FocusProject cleans up previous watches.
func TestFocusProjectCleansUpOnSwitch(t *testing.T) {
	projDir1 := t.TempDir()
	thoughtsDir1 := filepath.Join(projDir1, "thoughts")
	os.MkdirAll(thoughtsDir1, 0o755)

	projDir2 := t.TempDir()
	thoughtsDir2 := filepath.Join(projDir2, "thoughts")
	os.MkdirAll(thoughtsDir2, 0o755)

	proj1 := discovery.Project{
		Name: "proj1", Path: projDir1,
		Sources: []discovery.FileSource{{
			Name: "thoughts", Type: "tree", SourceTypeName: "thoughts",
			RootPath: thoughtsDir1, Auto: true,
		}},
	}
	proj2 := discovery.Project{
		Name: "proj2", Path: projDir2,
		Sources: []discovery.FileSource{{
			Name: "thoughts", Type: "tree", SourceTypeName: "thoughts",
			RootPath: thoughtsDir2, Auto: true,
		}},
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{proj1, proj2})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	// Focus proj1
	w.FocusProject("proj1")
	assertWatched(t, w, thoughtsDir1, true, "after focusing proj1")

	// Switch to proj2 — proj1 should be unwatched
	w.FocusProject("proj2")
	assertWatched(t, w, thoughtsDir1, false, "after focusing proj2")
	assertWatched(t, w, thoughtsDir2, true, "after focusing proj2")
}

// E-PENPAL-FOCUS: verifies FocusFile watches only the file's parent directory.
func TestFocusFileWatchesOnlyFileDir(t *testing.T) {
	projDir := t.TempDir()
	thoughtsDir := filepath.Join(projDir, "thoughts")
	plansDir := filepath.Join(projDir, "thoughts", "plans")
	os.MkdirAll(plansDir, 0o755)
	// Create a file so the dir exists
	os.WriteFile(filepath.Join(plansDir, "design.md"), []byte("# Design"), 0o644)

	project := discovery.Project{
		Name: "proj", Path: projDir,
		Sources: []discovery.FileSource{{
			Name: "thoughts", Type: "tree", SourceTypeName: "thoughts",
			RootPath: thoughtsDir, Auto: true,
		}},
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{project})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	w.FocusFile("proj", "thoughts/plans/design.md", "")

	// Only the plans/ dir should be watched, not the whole thoughts/ tree
	assertWatched(t, w, plansDir, true, "file's parent dir")
	assertWatched(t, w, thoughtsDir, false, "thoughts root (should not be watched)")
}

// E-PENPAL-FOCUS: verifies switching from file focus to project focus expands watches.
func TestFocusFileSwitchToProject(t *testing.T) {
	projDir := t.TempDir()
	thoughtsDir := filepath.Join(projDir, "thoughts")
	plansDir := filepath.Join(projDir, "thoughts", "plans")
	os.MkdirAll(plansDir, 0o755)

	project := discovery.Project{
		Name: "proj", Path: projDir,
		Sources: []discovery.FileSource{{
			Name: "thoughts", Type: "tree", SourceTypeName: "thoughts",
			RootPath: thoughtsDir, Auto: true,
		}},
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{project})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	// Start with file focus
	w.FocusFile("proj", "thoughts/plans/design.md", "")
	assertWatched(t, w, plansDir, true, "file focus")

	// Switch to project focus — should now watch all sources
	w.FocusProject("proj")
	assertWatched(t, w, thoughtsDir, true, "project focus includes root")
	assertWatched(t, w, plansDir, true, "project focus includes subdirs")
}

// E-PENPAL-FOCUS: verifies ClearFocus removes all dynamic watches.
func TestClearFocusRemovesAllWatches(t *testing.T) {
	projDir := t.TempDir()
	thoughtsDir := filepath.Join(projDir, "thoughts")
	os.MkdirAll(thoughtsDir, 0o755)

	project := discovery.Project{
		Name: "proj", Path: projDir,
		Sources: []discovery.FileSource{{
			Name: "thoughts", Type: "tree", SourceTypeName: "thoughts",
			RootPath: thoughtsDir, Auto: true,
		}},
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{project})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	w.FocusProject("proj")
	assertWatched(t, w, thoughtsDir, true, "before clear")

	w.ClearFocus()
	assertWatched(t, w, thoughtsDir, false, "after clear")
}

// E-PENPAL-FOCUS: verifies windowFocuses map unions watches across multiple windows.
func TestWindowFocusUnionAcrossWindows(t *testing.T) {
	projDir1 := t.TempDir()
	thoughtsDir1 := filepath.Join(projDir1, "thoughts")
	os.MkdirAll(thoughtsDir1, 0o755)

	projDir2 := t.TempDir()
	thoughtsDir2 := filepath.Join(projDir2, "thoughts")
	os.MkdirAll(thoughtsDir2, 0o755)

	proj1 := discovery.Project{
		Name: "proj1", Path: projDir1,
		Sources: []discovery.FileSource{{
			Name: "thoughts", Type: "tree", SourceTypeName: "thoughts",
			RootPath: thoughtsDir1, Auto: true,
		}},
	}
	proj2 := discovery.Project{
		Name: "proj2", Path: projDir2,
		Sources: []discovery.FileSource{{
			Name: "thoughts", Type: "tree", SourceTypeName: "thoughts",
			RootPath: thoughtsDir2, Auto: true,
		}},
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{proj1, proj2})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	w.SetWindowFocusProject("win-a", "proj1")
	w.SetWindowFocusProject("win-b", "proj2")

	assertWatched(t, w, thoughtsDir1, true, "window A keeps proj1 watched")
	assertWatched(t, w, thoughtsDir2, true, "window B keeps proj2 watched")

	w.ClearWindowFocus("win-a")

	assertWatched(t, w, thoughtsDir1, false, "clearing window A removes proj1 watch")
	assertWatched(t, w, thoughtsDir2, true, "window B still keeps proj2 watched")
}

func assertWatched(t *testing.T, w *Watcher, dir string, expected bool, context string) {
	t.Helper()
	watched := w.watcher.WatchList()
	found := false
	for _, p := range watched {
		if p == dir {
			found = true
			break
		}
	}
	if found != expected {
		if expected {
			t.Errorf("%s: expected %s to be watched, but it wasn't. watched: %v", context, dir, watched)
		} else {
			t.Errorf("%s: expected %s to NOT be watched, but it was", context, dir)
		}
	}
}
