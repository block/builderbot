package watcher

import (
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"testing"
	"time"

	"github.com/fsnotify/fsnotify"
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

// E-PENPAL-WORKTREE-WATCH: verifies syncBaseWatchesLocked watches .git/worktrees/ for
// projects that have worktrees, and that handleEvent triggers re-discovery on changes.
func TestWorktreeWatchDir(t *testing.T) {
	// Create a real git repo with a worktree so GitWorktreesDir resolves
	mainDir := t.TempDir()
	for _, args := range [][]string{
		{"init"},
		{"config", "user.email", "test@test.com"},
		{"config", "user.name", "Test"},
		{"commit", "--allow-empty", "-m", "init"},
	} {
		cmd := exec.Command("git", append([]string{"-C", mainDir}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	wtPath := filepath.Join(t.TempDir(), "wt1")
	cmd := exec.Command("git", "-C", mainDir, "worktree", "add", "-b", "b1", wtPath)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git worktree add: %v\n%s", err, out)
	}

	worktrees := discovery.DiscoverWorktrees(mainDir)
	project := discovery.Project{
		Name:      "myrepo",
		Path:      mainDir,
		Worktrees: worktrees,
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{project})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	// Sync base watches with the project
	w.focusMu.Lock()
	w.syncBaseWatchesLocked(nil, []discovery.Project{project})
	w.focusMu.Unlock()

	// The .git/worktrees/ dir should be watched
	gitWtDir := filepath.Join(mainDir, ".git", "worktrees")
	assertWatched(t, w, gitWtDir, true, ".git/worktrees/ should be base-watched")

	// Verify it's tracked in worktreeWatchDirs
	if _, ok := w.worktreeWatchDirs[filepath.Clean(gitWtDir)]; !ok {
		t.Errorf("expected %s in worktreeWatchDirs", gitWtDir)
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies that projects without worktrees get a .git/
// watch (to detect the first worktree add) but not a .git/worktrees/ watch.
func TestWorktreeWatchDir_NoWorktrees(t *testing.T) {
	mainDir := t.TempDir()
	for _, args := range [][]string{
		{"init"},
		{"config", "user.email", "test@test.com"},
		{"config", "user.name", "Test"},
		{"commit", "--allow-empty", "-m", "init"},
	} {
		cmd := exec.Command("git", append([]string{"-C", mainDir}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}

	project := discovery.Project{
		Name: "solo",
		Path: mainDir,
		// No worktrees
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{project})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	w.focusMu.Lock()
	w.syncBaseWatchesLocked(nil, []discovery.Project{project})
	w.focusMu.Unlock()

	if len(w.worktreeWatchDirs) != 0 {
		t.Errorf("expected no worktreeWatchDirs, got %v", w.worktreeWatchDirs)
	}

	// The .git/ dir should be watched to detect first worktree creation
	gitDir := filepath.Join(mainDir, ".git")
	assertWatched(t, w, gitDir, true, ".git/ should be watched for first-worktree detection")
	if _, ok := w.gitDirWatches[filepath.Clean(gitDir)]; !ok {
		t.Errorf("expected %s in gitDirWatches", gitDir)
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies that an event in .git/worktrees/ triggers re-discovery.
func TestWorktreeWatchDir_EventTriggersRediscovery(t *testing.T) {
	// Set up a watcher with a worktreeWatchDir and a discoverFn
	c := cache.New()
	c.SetProjects([]discovery.Project{{Name: "proj", Path: "/tmp/proj"}})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	discovered := make(chan struct{}, 1)
	w.discoverFn = func() ([]discovery.Project, error) {
		select {
		case discovered <- struct{}{}:
		default:
		}
		return []discovery.Project{{Name: "proj", Path: "/tmp/proj"}}, nil
	}
	w.workspacePaths = nil

	// Manually set a worktree watch dir
	fakeWtDir := t.TempDir()
	w.worktreeWatchDirs = map[string]struct{}{
		filepath.Clean(fakeWtDir): {},
	}

	// Simulate an event in the worktree watch dir
	fakeEvent := fsnotify.Event{
		Name: filepath.Join(fakeWtDir, "new-worktree"),
		Op:   fsnotify.Create,
	}
	w.handleEvent(fakeEvent)

	// The debounce timer fires after 100ms; wait for the discovery callback
	select {
	case <-discovered:
		// success
	case <-time.After(500 * time.Millisecond):
		t.Fatal("expected discoverFn to be called after worktree dir event")
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies that creating a "worktrees" entry in a watched
// .git/ dir triggers re-discovery (first worktree add scenario).
func TestWorktreeWatchDir_FirstWorktreeCreation(t *testing.T) {
	c := cache.New()
	c.SetProjects([]discovery.Project{{Name: "proj", Path: "/tmp/proj"}})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	discovered := make(chan struct{}, 1)
	w.discoverFn = func() ([]discovery.Project, error) {
		select {
		case discovered <- struct{}{}:
		default:
		}
		return []discovery.Project{{Name: "proj", Path: "/tmp/proj"}}, nil
	}
	w.workspacePaths = nil

	fakeGitDir := t.TempDir()
	w.focusMu.Lock()
	w.gitDirWatches = map[string]struct{}{
		filepath.Clean(fakeGitDir): {},
	}
	w.focusMu.Unlock()

	// A non-"worktrees" event in .git/ should NOT trigger rediscovery
	w.handleEvent(fsnotify.Event{
		Name: filepath.Join(fakeGitDir, "FETCH_HEAD"),
		Op:   fsnotify.Write,
	})

	select {
	case <-discovered:
		t.Fatal("should not trigger rediscovery for non-worktrees events in .git/")
	case <-time.After(200 * time.Millisecond):
		// good
	}

	// A Create event for "worktrees" in .git/ SHOULD trigger rediscovery
	w.handleEvent(fsnotify.Event{
		Name: filepath.Join(fakeGitDir, "worktrees"),
		Op:   fsnotify.Create,
	})

	select {
	case <-discovered:
		// success
	case <-time.After(500 * time.Millisecond):
		t.Fatal("expected discoverFn to be called when worktrees dir is created in .git/")
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies that rediscoverProjects updates the cache with
// new project data from discoverFn and broadcasts a projects-changed event.
func TestRediscoverProjects_UpdatesCache(t *testing.T) {
	c := cache.New()
	original := discovery.Project{Name: "repo", Path: "/tmp/repo"}
	c.SetProjects([]discovery.Project{original})

	w, err := New(c, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer w.Stop()

	updated := discovery.Project{
		Name: "repo", Path: "/tmp/repo",
		Worktrees: []discovery.Worktree{
			{Name: "repo", Path: "/tmp/repo", Branch: "main", IsMain: true},
			{Name: "feature", Path: "/tmp/wt/feature", Branch: "feature", IsMain: false},
		},
	}
	w.discoverFn = func() ([]discovery.Project, error) {
		return []discovery.Project{updated}, nil
	}

	events := w.Subscribe()
	defer w.Unsubscribe(events)

	w.rediscoverProjects()

	// Verify cache was updated with new worktree data
	projects := c.Projects()
	if len(projects) != 1 {
		t.Fatalf("expected 1 project, got %d", len(projects))
	}
	if len(projects[0].Worktrees) != 2 {
		t.Errorf("expected 2 worktrees in cache, got %d", len(projects[0].Worktrees))
	}

	// Verify a projects-changed event was broadcast
	select {
	case evt := <-events:
		if evt.Type != EventProjectsChanged {
			t.Errorf("expected EventProjectsChanged, got %v", evt.Type)
		}
	case <-time.After(500 * time.Millisecond):
		t.Fatal("expected projects-changed event to be broadcast")
	}
}

// E-PENPAL-WATCHER: verifies that non-.md file events are filtered out
// and do not trigger project rescans.
func TestEventFilter_NonMdFilesIgnored(t *testing.T) {
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

	events := w.Subscribe()
	defer w.Unsubscribe(events)

	// Focus the project so the thoughts dir is watched and events reach the filter
	w.FocusProject("proj")

	// Create event for a .js file should NOT trigger a files-changed broadcast
	w.handleEvent(fsnotify.Event{
		Name: filepath.Join(thoughtsDir, "script.js"),
		Op:   fsnotify.Create,
	})

	select {
	case evt := <-events:
		t.Fatalf("expected no event for .js file, got %v", evt)
	case <-time.After(200 * time.Millisecond):
		// good — no event
	}

	// Create event for a .md file SHOULD trigger a files-changed broadcast
	w.handleEvent(fsnotify.Event{
		Name: filepath.Join(thoughtsDir, "notes.md"),
		Op:   fsnotify.Create,
	})

	select {
	case evt := <-events:
		if evt.Type != EventFilesChanged {
			t.Errorf("expected EventFilesChanged, got %v", evt.Type)
		}
	case <-time.After(500 * time.Millisecond):
		t.Fatal("expected files-changed event for .md file")
	}
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
