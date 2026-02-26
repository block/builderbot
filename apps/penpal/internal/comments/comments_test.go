package comments

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/discovery"
)

const testProject = "testproj"

// newTestStore sets up a Store backed by a temp directory with a fake project.
// The returned cleanup function removes the temp directory.
func newTestStore(t *testing.T) *Store {
	t.Helper()

	tmpDir := t.TempDir()

	// Create the project directory structure that discovery expects:
	// {root}/{project}/thoughts/
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

	return NewStore(c, nil)
}

func TestCreateThread(t *testing.T) {
	store := newTestStore(t)

	anchor := Anchor{
		SelectedText: "some important text",
		HeadingPath:  "## Introduction",
	}
	comment := Comment{
		Author: "alice",
		Role:   "human",
		Body:   "This needs clarification.",
	}

	thread, err := store.CreateThread(testProject, "doc.md", anchor, comment)
	if err != nil {
		t.Fatalf("CreateThread: %v", err)
	}

	if thread.ID == "" {
		t.Error("expected thread to have an ID")
	}
	if thread.Status != "open" {
		t.Errorf("expected status 'open', got %q", thread.Status)
	}
	if thread.CreatedAt.IsZero() {
		t.Error("expected CreatedAt to be set")
	}
	if len(thread.Comments) != 1 {
		t.Fatalf("expected 1 comment, got %d", len(thread.Comments))
	}
	if thread.Comments[0].ID == "" {
		t.Error("expected comment to have an ID")
	}
	if thread.Comments[0].Author != "alice" {
		t.Errorf("expected author 'alice', got %q", thread.Comments[0].Author)
	}
	if thread.Comments[0].Body != "This needs clarification." {
		t.Errorf("unexpected comment body: %q", thread.Comments[0].Body)
	}
	if thread.Comments[0].CreatedAt.IsZero() {
		t.Error("expected comment CreatedAt to be set")
	}
	if thread.Anchor.SelectedText != "some important text" {
		t.Errorf("unexpected anchor text: %q", thread.Anchor.SelectedText)
	}
}

func TestAddComment(t *testing.T) {
	store := newTestStore(t)

	anchor := Anchor{SelectedText: "text"}
	first := Comment{Author: "alice", Role: "human", Body: "First comment"}
	thread, err := store.CreateThread(testProject, "doc.md", anchor, first)
	if err != nil {
		t.Fatalf("CreateThread: %v", err)
	}

	second := Comment{Author: "bot", Role: "agent", Body: "I can help with that."}
	updated, err := store.AddComment(testProject, "doc.md", thread.ID, second)
	if err != nil {
		t.Fatalf("AddComment: %v", err)
	}

	if len(updated.Comments) != 2 {
		t.Fatalf("expected 2 comments, got %d", len(updated.Comments))
	}
	if updated.Comments[1].Author != "bot" {
		t.Errorf("expected author 'bot', got %q", updated.Comments[1].Author)
	}
	if updated.Comments[1].ID == "" {
		t.Error("expected second comment to have an ID")
	}
	if updated.Comments[1].ID == updated.Comments[0].ID {
		t.Error("expected different IDs for each comment")
	}
}

func TestAddCommentThreadNotFound(t *testing.T) {
	store := newTestStore(t)

	_, err := store.AddComment(testProject, "doc.md", "nonexistent", Comment{
		Author: "alice", Role: "human", Body: "Hello",
	})
	if err == nil {
		t.Fatal("expected error for nonexistent thread")
	}
}

func TestResolveAndReopenThread(t *testing.T) {
	store := newTestStore(t)

	anchor := Anchor{SelectedText: "text"}
	comment := Comment{Author: "alice", Role: "human", Body: "Fix this"}
	thread, err := store.CreateThread(testProject, "doc.md", anchor, comment)
	if err != nil {
		t.Fatalf("CreateThread: %v", err)
	}

	// Resolve
	if err := store.ResolveThread(testProject, "doc.md", thread.ID, "bob"); err != nil {
		t.Fatalf("ResolveThread: %v", err)
	}

	threads, err := store.LoadThreads(testProject, "doc.md")
	if err != nil {
		t.Fatalf("LoadThreads: %v", err)
	}
	if len(threads) != 1 {
		t.Fatalf("expected 1 thread, got %d", len(threads))
	}
	if threads[0].Status != "resolved" {
		t.Errorf("expected status 'resolved', got %q", threads[0].Status)
	}
	if threads[0].ResolvedBy != "bob" {
		t.Errorf("expected resolvedBy 'bob', got %q", threads[0].ResolvedBy)
	}
	if threads[0].ResolvedAt.IsZero() {
		t.Error("expected ResolvedAt to be set")
	}

	// Reopen
	if err := store.ReopenThread(testProject, "doc.md", thread.ID); err != nil {
		t.Fatalf("ReopenThread: %v", err)
	}

	threads, err = store.LoadThreads(testProject, "doc.md")
	if err != nil {
		t.Fatalf("LoadThreads: %v", err)
	}
	if threads[0].Status != "open" {
		t.Errorf("expected status 'open', got %q", threads[0].Status)
	}
	if threads[0].ResolvedBy != "" {
		t.Errorf("expected resolvedBy to be cleared, got %q", threads[0].ResolvedBy)
	}
	if !threads[0].ResolvedAt.IsZero() {
		t.Error("expected ResolvedAt to be cleared")
	}
}

func TestLoadSaveRoundTrip(t *testing.T) {
	store := newTestStore(t)

	// Create a thread with a comment
	anchor := Anchor{
		SelectedText: "round-trip text",
		Before:       "before context",
		After:        "after context",
		HeadingPath:  "## Section > ### Subsection",
	}
	comment := Comment{Author: "alice", Role: "human", Body: "Testing persistence"}
	thread, err := store.CreateThread(testProject, "subdir/nested.md", anchor, comment)
	if err != nil {
		t.Fatalf("CreateThread: %v", err)
	}

	// Load and verify
	fc, err := store.Load(testProject, "subdir/nested.md")
	if err != nil {
		t.Fatalf("Load: %v", err)
	}

	if len(fc.Threads) != 1 {
		t.Fatalf("expected 1 thread, got %d", len(fc.Threads))
	}

	loaded := fc.Threads[0]
	if loaded.ID != thread.ID {
		t.Errorf("ID mismatch: %q vs %q", loaded.ID, thread.ID)
	}
	if loaded.Status != thread.Status {
		t.Errorf("Status mismatch: %q vs %q", loaded.Status, thread.Status)
	}
	if loaded.Anchor.SelectedText != anchor.SelectedText {
		t.Errorf("Anchor.SelectedText mismatch: %q vs %q", loaded.Anchor.SelectedText, anchor.SelectedText)
	}
	if loaded.Anchor.Before != anchor.Before {
		t.Errorf("Anchor.Before mismatch: %q vs %q", loaded.Anchor.Before, anchor.Before)
	}
	if loaded.Anchor.After != anchor.After {
		t.Errorf("Anchor.After mismatch: %q vs %q", loaded.Anchor.After, anchor.After)
	}
	if loaded.Anchor.HeadingPath != anchor.HeadingPath {
		t.Errorf("Anchor.HeadingPath mismatch: %q vs %q", loaded.Anchor.HeadingPath, anchor.HeadingPath)
	}
	if len(loaded.Comments) != 1 {
		t.Fatalf("expected 1 comment, got %d", len(loaded.Comments))
	}
	if loaded.Comments[0].Body != "Testing persistence" {
		t.Errorf("comment body mismatch: %q", loaded.Comments[0].Body)
	}
}

func TestLoadNonexistentReturnsEmpty(t *testing.T) {
	store := newTestStore(t)

	fc, err := store.Load(testProject, "does-not-exist.md")
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if fc == nil {
		t.Fatal("expected non-nil FileComments")
	}
	if len(fc.Threads) != 0 {
		t.Errorf("expected 0 threads, got %d", len(fc.Threads))
	}
}

func TestLoadProjectNotFound(t *testing.T) {
	store := newTestStore(t)

	_, err := store.Load("nonexistent-project", "doc.md")
	if err == nil {
		t.Fatal("expected error for nonexistent project")
	}
}

func TestListOpenThreadsAcrossFiles(t *testing.T) {
	store := newTestStore(t)

	// Create threads in different files
	anchor := Anchor{SelectedText: "text"}

	// File 1: one open thread
	_, err := store.CreateThread(testProject, "file1.md", anchor, Comment{
		Author: "alice", Role: "human", Body: "Comment on file1",
	})
	if err != nil {
		t.Fatalf("CreateThread file1: %v", err)
	}

	// File 2: two threads - one open, one resolved
	t2, err := store.CreateThread(testProject, "file2.md", anchor, Comment{
		Author: "alice", Role: "human", Body: "First thread on file2",
	})
	if err != nil {
		t.Fatalf("CreateThread file2 first: %v", err)
	}
	_, err = store.CreateThread(testProject, "file2.md", anchor, Comment{
		Author: "alice", Role: "human", Body: "Second thread on file2",
	})
	if err != nil {
		t.Fatalf("CreateThread file2 second: %v", err)
	}
	if err := store.ResolveThread(testProject, "file2.md", t2.ID, "bob"); err != nil {
		t.Fatalf("ResolveThread: %v", err)
	}

	// File 3 in subdirectory: one open thread
	_, err = store.CreateThread(testProject, "sub/file3.md", anchor, Comment{
		Author: "alice", Role: "human", Body: "Comment on file3",
	})
	if err != nil {
		t.Fatalf("CreateThread file3: %v", err)
	}

	open, err := store.ListOpenThreads(testProject)
	if err != nil {
		t.Fatalf("ListOpenThreads: %v", err)
	}

	// Expect 3 open threads: 1 from file1, 1 from file2 (second), 1 from file3
	if len(open) != 3 {
		t.Fatalf("expected 3 open threads, got %d", len(open))
	}

	// Verify file paths are present
	filePaths := make(map[string]int)
	for _, twf := range open {
		filePaths[twf.FilePath]++
	}
	if filePaths["file1.md"] != 1 {
		t.Errorf("expected 1 open thread from file1.md, got %d", filePaths["file1.md"])
	}
	if filePaths["file2.md"] != 1 {
		t.Errorf("expected 1 open thread from file2.md, got %d", filePaths["file2.md"])
	}
	if filePaths[filepath.Join("sub", "file3.md")] != 1 {
		t.Errorf("expected 1 open thread from sub/file3.md, got %d", filePaths[filepath.Join("sub", "file3.md")])
	}
}

func TestListOpenThreadsEmptyProject(t *testing.T) {
	store := newTestStore(t)

	open, err := store.ListOpenThreads(testProject)
	if err != nil {
		t.Fatalf("ListOpenThreads: %v", err)
	}
	if open != nil {
		t.Errorf("expected nil, got %d threads", len(open))
	}
}

func TestListFilesInReview(t *testing.T) {
	store := newTestStore(t)

	anchor := Anchor{SelectedText: "text"}

	// File 1: one open thread → in review
	_, err := store.CreateThread(testProject, "file1.md", anchor, Comment{
		Author: "alice", Role: "human", Body: "Comment on file1",
	})
	if err != nil {
		t.Fatalf("CreateThread file1: %v", err)
	}

	// File 2: one thread, but resolved → NOT in review
	t2, err := store.CreateThread(testProject, "file2.md", anchor, Comment{
		Author: "bob", Role: "agent", Body: "Comment on file2",
	})
	if err != nil {
		t.Fatalf("CreateThread file2: %v", err)
	}
	if err := store.ResolveThread(testProject, "file2.md", t2.ID, "bob"); err != nil {
		t.Fatalf("ResolveThread file2: %v", err)
	}

	// File 3: one open thread → in review
	_, err = store.CreateThread(testProject, "file3.md", anchor, Comment{
		Author: "charlie", Role: "human", Body: "Comment on file3",
	})
	if err != nil {
		t.Fatalf("CreateThread file3: %v", err)
	}

	files, err := store.ListFilesInReview(testProject)
	if err != nil {
		t.Fatalf("ListFilesInReview: %v", err)
	}

	if len(files) != 2 {
		t.Fatalf("expected 2 files in review, got %d", len(files))
	}

	byFile := make(map[string]FileInReview)
	for _, f := range files {
		byFile[f.FilePath] = f
	}

	f1, ok := byFile["file1.md"]
	if !ok {
		t.Fatal("expected file1.md to be in review")
	}
	if f1.OpenThreads != 1 {
		t.Errorf("expected 1 open thread on file1.md, got %d", f1.OpenThreads)
	}

	if _, ok := byFile["file2.md"]; ok {
		t.Error("file2.md should NOT be in review (all threads resolved)")
	}

	f3, ok := byFile["file3.md"]
	if !ok {
		t.Fatal("expected file3.md to be in review")
	}
	if f3.OpenThreads != 1 {
		t.Errorf("expected 1 open thread on file3.md, got %d", f3.OpenThreads)
	}
}

func TestFilesInReviewDerivedFromOpenThreads(t *testing.T) {
	store := newTestStore(t)

	anchor := Anchor{SelectedText: "text"}

	// Create threads on two files
	t1, err := store.CreateThread(testProject, "doc1.md", anchor, Comment{
		Author: "alice", Role: "human", Body: "Comment 1",
	})
	if err != nil {
		t.Fatalf("CreateThread doc1: %v", err)
	}
	_, err = store.CreateThread(testProject, "doc2.md", anchor, Comment{
		Author: "bob", Role: "human", Body: "Comment 2",
	})
	if err != nil {
		t.Fatalf("CreateThread doc2: %v", err)
	}

	// Both should be in review
	files, err := store.ListFilesInReview(testProject)
	if err != nil {
		t.Fatalf("ListFilesInReview: %v", err)
	}
	if len(files) != 2 {
		t.Fatalf("expected 2 files in review, got %d", len(files))
	}

	// Resolve all threads on doc1
	if err := store.ResolveThread(testProject, "doc1.md", t1.ID, "alice"); err != nil {
		t.Fatalf("ResolveThread: %v", err)
	}

	// Only doc2 should remain in review
	files, err = store.ListFilesInReview(testProject)
	if err != nil {
		t.Fatalf("ListFilesInReview after resolve: %v", err)
	}
	if len(files) != 1 {
		t.Fatalf("expected 1 file in review, got %d", len(files))
	}
	if files[0].FilePath != "doc2.md" {
		t.Errorf("expected doc2.md in review, got %q", files[0].FilePath)
	}

	// Reopen the thread on doc1
	if err := store.ReopenThread(testProject, "doc1.md", t1.ID); err != nil {
		t.Fatalf("ReopenThread: %v", err)
	}

	// Both should be in review again
	files, err = store.ListFilesInReview(testProject)
	if err != nil {
		t.Fatalf("ListFilesInReview after reopen: %v", err)
	}
	if len(files) != 2 {
		t.Fatalf("expected 2 files in review after reopen, got %d", len(files))
	}
}

func TestMigrateInReplyToOnSave(t *testing.T) {
	store := newTestStore(t)

	// Write legacy JSON without inReplyTo fields
	legacy := &FileComments{
		Threads: []Thread{{
			ID:     "t1",
			Status: "open",
			Anchor: Anchor{SelectedText: "text"},
			Comments: []Comment{
				{ID: "c1", Author: "alice", Role: "human", Body: "First"},
				{ID: "c2", Author: "bob", Role: "agent", Body: "Second"},
				{ID: "c3", Author: "alice", Role: "human", Body: "Third"},
			},
		}},
	}

	// Save — should trigger migration
	if err := store.Save(testProject, "legacy.md", legacy); err != nil {
		t.Fatalf("Save: %v", err)
	}

	// Load the raw JSON and verify inReplyTo was backfilled
	p, _ := store.commentsPath(testProject, "legacy.md")
	data, err := os.ReadFile(p)
	if err != nil {
		t.Fatalf("ReadFile: %v", err)
	}

	var fc FileComments
	if err := json.Unmarshal(data, &fc); err != nil {
		t.Fatalf("Unmarshal: %v", err)
	}

	comments := fc.Threads[0].Comments
	if comments[0].InReplyTo != "" {
		t.Errorf("first comment should have no inReplyTo, got %q", comments[0].InReplyTo)
	}
	if comments[1].InReplyTo != "c1" {
		t.Errorf("second comment inReplyTo should be 'c1', got %q", comments[1].InReplyTo)
	}
	if comments[2].InReplyTo != "c2" {
		t.Errorf("third comment inReplyTo should be 'c2', got %q", comments[2].InReplyTo)
	}
}

func TestMigratePreservesExistingInReplyTo(t *testing.T) {
	store := newTestStore(t)

	// Comments where c2 already has inReplyTo set
	fc := &FileComments{
		Threads: []Thread{{
			ID:     "t1",
			Status: "open",
			Anchor: Anchor{SelectedText: "text"},
			Comments: []Comment{
				{ID: "c1", Author: "alice", Role: "human", Body: "First"},
				{ID: "c2", Author: "bob", Role: "agent", Body: "Second", InReplyTo: "c1"},
				{ID: "c3", Author: "alice", Role: "human", Body: "Third"},
			},
		}},
	}

	if err := store.Save(testProject, "existing.md", fc); err != nil {
		t.Fatalf("Save: %v", err)
	}

	loaded, err := store.Load(testProject, "existing.md")
	if err != nil {
		t.Fatalf("Load: %v", err)
	}

	comments := loaded.Threads[0].Comments
	if comments[1].InReplyTo != "c1" {
		t.Errorf("c2 inReplyTo should remain 'c1', got %q", comments[1].InReplyTo)
	}
	if comments[2].InReplyTo != "c2" {
		t.Errorf("c3 inReplyTo should be backfilled to 'c2', got %q", comments[2].InReplyTo)
	}
}

func TestAddCommentSetsInReplyTo(t *testing.T) {
	store := newTestStore(t)

	anchor := Anchor{SelectedText: "text"}
	first := Comment{Author: "alice", Role: "human", Body: "First"}
	thread, err := store.CreateThread(testProject, "reply.md", anchor, first)
	if err != nil {
		t.Fatalf("CreateThread: %v", err)
	}

	second := Comment{Author: "bot", Role: "agent", Body: "Reply"}
	updated, err := store.AddComment(testProject, "reply.md", thread.ID, second)
	if err != nil {
		t.Fatalf("AddComment: %v", err)
	}

	if updated.Comments[1].InReplyTo != updated.Comments[0].ID {
		t.Errorf("reply InReplyTo should be %q, got %q",
			updated.Comments[0].ID, updated.Comments[1].InReplyTo)
	}
}

func TestLegacyJSONWithoutInReplyToLoads(t *testing.T) {
	store := newTestStore(t)

	// Manually write JSON without inReplyTo fields (simulating legacy data)
	legacyJSON := `{
		"threads": [{
			"id": "t1",
			"status": "open",
			"anchor": {"selectedText": "text"},
			"comments": [
				{"id": "c1", "author": "alice", "role": "human", "body": "Hello", "createdAt": "2025-01-01T00:00:00Z"},
				{"id": "c2", "author": "bob", "role": "agent", "body": "World", "createdAt": "2025-01-01T00:01:00Z"}
			],
			"createdAt": "2025-01-01T00:00:00Z"
		}]
	}`

	p, _ := store.commentsPath(testProject, "old.md")
	dir := filepath.Dir(p)
	if err := os.MkdirAll(dir, 0755); err != nil {
		t.Fatalf("MkdirAll: %v", err)
	}
	if err := os.WriteFile(p, []byte(legacyJSON), 0644); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}

	fc, err := store.Load(testProject, "old.md")
	if err != nil {
		t.Fatalf("Load: %v", err)
	}

	if len(fc.Threads) != 1 {
		t.Fatalf("expected 1 thread, got %d", len(fc.Threads))
	}
	if len(fc.Threads[0].Comments) != 2 {
		t.Fatalf("expected 2 comments, got %d", len(fc.Threads[0].Comments))
	}
	// InReplyTo should be empty (not backfilled until Save)
	if fc.Threads[0].Comments[0].InReplyTo != "" {
		t.Errorf("expected empty InReplyTo for first comment, got %q", fc.Threads[0].Comments[0].InReplyTo)
	}
	if fc.Threads[0].Comments[1].InReplyTo != "" {
		t.Errorf("expected empty InReplyTo for second comment (not yet migrated), got %q", fc.Threads[0].Comments[1].InReplyTo)
	}
}
