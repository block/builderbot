package comments

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"testing"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/discovery"
)

// E-PENPAL-THREAD-MUTEX: exercises concurrent writes to the same store.
// Multiple goroutines create threads and add comments simultaneously.
// Verifies no data corruption: all operations succeed and the persisted file is valid JSON.
func TestConcurrentThreadCreation(t *testing.T) {
	tmpDir := t.TempDir()
	projectDir := filepath.Join(tmpDir, testProject)
	thoughtsDir := filepath.Join(projectDir, "thoughts")
	if err := os.MkdirAll(thoughtsDir, 0755); err != nil {
		t.Fatalf("creating thoughts dir: %v", err)
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{
		{
			Name: testProject,
			Path: projectDir,
			Sources: []discovery.FileSource{{
				Name:     "thoughts",
				Type:     "thoughts",
				RootPath: thoughtsDir,
				Auto:     true,
			}},
		},
	})
	store := NewStore(c, nil)

	const numGoroutines = 20
	var wg sync.WaitGroup
	errs := make(chan error, numGoroutines)

	// All goroutines create threads on the same file concurrently
	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			anchor := Anchor{SelectedText: fmt.Sprintf("text-%d", idx)}
			comment := Comment{
				Author: fmt.Sprintf("user-%d", idx),
				Role:   "human",
				Body:   fmt.Sprintf("Comment %d", idx),
			}
			_, err := store.CreateThread(testProject, "concurrent.md", anchor, comment)
			if err != nil {
				errs <- fmt.Errorf("goroutine %d CreateThread: %w", idx, err)
			}
		}(i)
	}

	wg.Wait()
	close(errs)

	for err := range errs {
		t.Error(err)
	}

	// Verify persisted file is valid JSON and has all threads
	fc, err := store.Load(testProject, "concurrent.md")
	if err != nil {
		t.Fatalf("Load after concurrent writes: %v", err)
	}
	if len(fc.Threads) != numGoroutines {
		t.Errorf("expected %d threads, got %d", numGoroutines, len(fc.Threads))
	}

	// Verify the raw file on disk is valid JSON
	p, err := store.commentsPath(testProject, "concurrent.md")
	if err != nil {
		t.Fatalf("commentsPath: %v", err)
	}
	data, err := os.ReadFile(p)
	if err != nil {
		t.Fatalf("ReadFile: %v", err)
	}
	var raw FileComments
	if err := json.Unmarshal(data, &raw); err != nil {
		t.Fatalf("persisted JSON is invalid: %v", err)
	}
	if len(raw.Threads) != numGoroutines {
		t.Errorf("raw JSON has %d threads, expected %d", len(raw.Threads), numGoroutines)
	}
}

// E-PENPAL-THREAD-MUTEX: exercises concurrent thread creation AND comment addition.
// Verifies no data corruption when mixing operations on the same file.
func TestConcurrentCreateAndAddComment(t *testing.T) {
	tmpDir := t.TempDir()
	projectDir := filepath.Join(tmpDir, testProject)
	thoughtsDir := filepath.Join(projectDir, "thoughts")
	if err := os.MkdirAll(thoughtsDir, 0755); err != nil {
		t.Fatalf("creating thoughts dir: %v", err)
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{
		{
			Name: testProject,
			Path: projectDir,
			Sources: []discovery.FileSource{{
				Name:     "thoughts",
				Type:     "thoughts",
				RootPath: thoughtsDir,
				Auto:     true,
			}},
		},
	})
	store := NewStore(c, nil)

	// First create a seed thread to add comments to
	seedAnchor := Anchor{SelectedText: "seed text"}
	seedComment := Comment{Author: "seed", Role: "human", Body: "Seed comment"}
	seedThread, err := store.CreateThread(testProject, "mixed.md", seedAnchor, seedComment)
	if err != nil {
		t.Fatalf("CreateThread seed: %v", err)
	}

	const numGoroutines = 10
	var wg sync.WaitGroup
	errs := make(chan error, numGoroutines*2)

	// Half create new threads, half add comments to the seed thread
	for i := 0; i < numGoroutines; i++ {
		wg.Add(2)
		go func(idx int) {
			defer wg.Done()
			anchor := Anchor{SelectedText: fmt.Sprintf("new-%d", idx)}
			comment := Comment{
				Author: fmt.Sprintf("creator-%d", idx),
				Role:   "human",
				Body:   fmt.Sprintf("New thread %d", idx),
			}
			_, err := store.CreateThread(testProject, "mixed.md", anchor, comment)
			if err != nil {
				errs <- fmt.Errorf("goroutine %d CreateThread: %w", idx, err)
			}
		}(i)
		go func(idx int) {
			defer wg.Done()
			comment := Comment{
				Author: fmt.Sprintf("commenter-%d", idx),
				Role:   "agent",
				Body:   fmt.Sprintf("Reply %d", idx),
			}
			_, err := store.AddComment(testProject, "mixed.md", seedThread.ID, comment)
			if err != nil {
				errs <- fmt.Errorf("goroutine %d AddComment: %w", idx, err)
			}
		}(i)
	}

	wg.Wait()
	close(errs)

	for err := range errs {
		t.Error(err)
	}

	// Verify: 1 seed + numGoroutines new threads
	fc, err := store.Load(testProject, "mixed.md")
	if err != nil {
		t.Fatalf("Load after concurrent mixed ops: %v", err)
	}
	expectedThreads := 1 + numGoroutines
	if len(fc.Threads) != expectedThreads {
		t.Errorf("expected %d threads, got %d", expectedThreads, len(fc.Threads))
	}

	// The seed thread should have 1 original + numGoroutines added comments
	for _, thread := range fc.Threads {
		if thread.ID == seedThread.ID {
			expectedComments := 1 + numGoroutines
			if len(thread.Comments) != expectedComments {
				t.Errorf("seed thread: expected %d comments, got %d", expectedComments, len(thread.Comments))
			}
			return
		}
	}
	t.Error("seed thread not found in results")
}
