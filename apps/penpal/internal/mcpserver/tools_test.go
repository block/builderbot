package mcpserver

import (
	"context"
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/discovery"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

type testEnv struct {
	session  *mcp.ClientSession
	store    *comments.Store
	cache    *cache.Cache
	projDir  string
	projName string
}

func setup(t *testing.T) (*testEnv, func()) {
	t.Helper()

	projDir := t.TempDir()
	projName := "test-project"

	c := cache.New()
	act := activity.New()
	cs := comments.NewStore(c, act)

	c.SetProjects([]discovery.Project{{
		Name:   projName,
		Path:   projDir,
		Origin: "standalone",
	}})

	handler := NewHandler(cs, c)
	ts := httptest.NewServer(handler)

	ctx := context.Background()
	transport := &mcp.StreamableClientTransport{
		Endpoint:   ts.URL,
		MaxRetries: -1,
	}
	client := mcp.NewClient(&mcp.Implementation{
		Name:    "test-client",
		Version: "0.0.1",
	}, nil)
	session, err := client.Connect(ctx, transport, nil)
	if err != nil {
		ts.Close()
		t.Fatalf("connecting MCP client: %v", err)
	}

	env := &testEnv{
		session:  session,
		store:    cs,
		cache:    c,
		projDir:  projDir,
		projName: projName,
	}
	cleanup := func() {
		session.Close()
		ts.Close()
	}
	return env, cleanup
}

func callTool(t *testing.T, env *testEnv, name string, args map[string]any) string {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	res, err := env.session.CallTool(ctx, &mcp.CallToolParams{
		Name:      name,
		Arguments: args,
	})
	if err != nil {
		t.Fatalf("CallTool %s: %v", name, err)
	}
	if len(res.Content) == 0 {
		t.Fatalf("CallTool %s: empty content", name)
	}
	tc, ok := res.Content[0].(*mcp.TextContent)
	if !ok {
		t.Fatalf("CallTool %s: expected TextContent, got %T", name, res.Content[0])
	}
	return tc.Text
}

func callToolExpectError(t *testing.T, env *testEnv, name string, args map[string]any) string {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	res, err := env.session.CallTool(ctx, &mcp.CallToolParams{
		Name:      name,
		Arguments: args,
	})
	if err != nil {
		return err.Error()
	}
	if !res.IsError {
		t.Fatalf("CallTool %s: expected IsError=true", name)
	}
	if len(res.Content) == 0 {
		return ""
	}
	tc, ok := res.Content[0].(*mcp.TextContent)
	if !ok {
		t.Fatalf("CallTool %s: expected TextContent in error, got %T", name, res.Content[0])
	}
	return tc.Text
}

func createTestThread(t *testing.T, env *testEnv, filePath, body string) *comments.Thread {
	t.Helper()

	// Ensure the source file exists on disk so ListFilesInReview doesn't skip it.
	srcPath := filepath.Join(env.projDir, filePath)
	if err := os.MkdirAll(filepath.Dir(srcPath), 0755); err != nil {
		t.Fatalf("creating parent dir for %s: %v", filePath, err)
	}
	if err := os.WriteFile(srcPath, []byte("test content"), 0644); err != nil {
		t.Fatalf("creating source file %s: %v", filePath, err)
	}

	anchor := comments.Anchor{SelectedText: "some text"}
	comment := comments.Comment{Author: "human", Role: "human", Body: body}
	thread, err := env.store.CreateThread(env.projName, filePath, anchor, comment)
	if err != nil {
		t.Fatalf("creating thread: %v", err)
	}
	return thread
}

// E-PENPAL-MCP-TOOLS: verifies penpal_find_project returns correct project for a directory.
func TestFindProject(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	text := callTool(t, env, "penpal_find_project", map[string]any{
		"directory": env.projDir,
	})

	var result map[string]string
	if err := json.Unmarshal([]byte(text), &result); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if result["project"] != env.projName {
		t.Errorf("project = %q, want %q", result["project"], env.projName)
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_find_project returns error for unknown directory.
func TestFindProject_NotFound(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	errText := callToolExpectError(t, env, "penpal_find_project", map[string]any{
		"directory": "/nonexistent/path",
	})
	if errText == "" {
		t.Fatal("expected non-empty error")
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_create_thread computes anchor context from disk.
func TestCreateThread(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	// Write a real file for anchor context computation
	mdPath := filepath.Join(env.projDir, "thoughts", "test.md")
	os.MkdirAll(filepath.Dir(mdPath), 0755)
	os.WriteFile(mdPath, []byte("# Hello World\n\nThis is a test document with content."), 0644)

	text := callTool(t, env, "penpal_create_thread", map[string]any{
		"project":      env.projName,
		"path":         "thoughts/test.md",
		"selectedText": "test document",
		"body":         "This needs revision.",
	})

	var thread comments.Thread
	if err := json.Unmarshal([]byte(text), &thread); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if thread.ID == "" {
		t.Error("thread ID is empty")
	}
	if thread.Anchor.Before == "" {
		t.Error("anchor before context is empty")
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_list_threads returns threads for a specific file.
func TestListThreads_ByFile(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	thread := createTestThread(t, env, "thoughts/list-test.md", "A comment")

	text := callTool(t, env, "penpal_list_threads", map[string]any{
		"project": env.projName,
		"path":    "thoughts/list-test.md",
	})

	var threads []comments.Thread
	if err := json.Unmarshal([]byte(text), &threads); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(threads) != 1 {
		t.Fatalf("threads = %d, want 1", len(threads))
	}
	if threads[0].ID != thread.ID {
		t.Errorf("ID = %q, want %q", threads[0].ID, thread.ID)
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_list_threads returns threads across all project files.
func TestListThreads_AcrossProject(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	createTestThread(t, env, "thoughts/file-a.md", "Comment A")
	createTestThread(t, env, "thoughts/file-b.md", "Comment B")

	text := callTool(t, env, "penpal_list_threads", map[string]any{
		"project": env.projName,
	})

	var threads []comments.ThreadWithFile
	if err := json.Unmarshal([]byte(text), &threads); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(threads) != 2 {
		t.Fatalf("threads = %d, want 2", len(threads))
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_read_thread returns full thread with comments.
func TestReadThread(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	thread := createTestThread(t, env, "thoughts/read-test.md", "Read me")

	text := callTool(t, env, "penpal_read_thread", map[string]any{
		"project":  env.projName,
		"path":     "thoughts/read-test.md",
		"threadId": thread.ID,
	})

	var result comments.Thread
	if err := json.Unmarshal([]byte(text), &result); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if result.Comments[0].Body != "Read me" {
		t.Errorf("body = %q, want %q", result.Comments[0].Body, "Read me")
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_read_thread returns error for nonexistent thread.
func TestReadThread_NotFound(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	createTestThread(t, env, "thoughts/read-nf.md", "body")

	errText := callToolExpectError(t, env, "penpal_read_thread", map[string]any{
		"project":  env.projName,
		"path":     "thoughts/read-nf.md",
		"threadId": "nonexistent-id",
	})
	if errText == "" {
		t.Fatal("expected non-empty error")
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_reply adds agent comment to thread.
func TestReply(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	thread := createTestThread(t, env, "thoughts/reply-test.md", "Original")

	text := callTool(t, env, "penpal_reply", map[string]any{
		"project":  env.projName,
		"path":     "thoughts/reply-test.md",
		"threadId": thread.ID,
		"body":     "My reply",
	})

	var result comments.Thread
	if err := json.Unmarshal([]byte(text), &result); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(result.Comments) != 2 {
		t.Fatalf("comments = %d, want 2", len(result.Comments))
	}
	if result.Comments[1].Role != "agent" {
		t.Errorf("role = %q, want %q", result.Comments[1].Role, "agent")
	}
}

// fileWithThreadsResponse matches the enriched response from penpal_files_in_review.
type fileWithThreadsResponse struct {
	FilePath      string            `json:"filePath"`
	OpenThreads   int               `json:"openThreads"`
	Threads       []comments.Thread `json:"threads,omitempty"`
	OldestPending *comments.Thread  `json:"oldestPending,omitempty"`
}

// E-PENPAL-MCP-TOOLS: verifies penpal_files_in_review returns files with open threads.
func TestFilesInReview(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	// Create source files so ListFilesInReview finds them
	for _, name := range []string{"thoughts/review-a.md", "thoughts/review-b.md"} {
		p := filepath.Join(env.projDir, name)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte("content"), 0o644); err != nil {
			t.Fatal(err)
		}
	}

	createTestThread(t, env, "thoughts/review-a.md", "Comment")
	createTestThread(t, env, "thoughts/review-b.md", "Comment")

	text := callTool(t, env, "penpal_files_in_review", map[string]any{
		"project": env.projName,
	})

	var files []fileWithThreadsResponse
	if err := json.Unmarshal([]byte(text), &files); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(files) != 2 {
		t.Fatalf("files = %d, want 2", len(files))
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_files_in_review includes open thread data in response.
func TestFilesInReview_IncludesThreads(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	thread := createTestThread(t, env, "thoughts/enriched.md", "Please review this")

	text := callTool(t, env, "penpal_files_in_review", map[string]any{
		"project": env.projName,
	})

	var files []fileWithThreadsResponse
	if err := json.Unmarshal([]byte(text), &files); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(files) != 1 {
		t.Fatalf("files = %d, want 1", len(files))
	}
	if len(files[0].Threads) != 1 {
		t.Fatalf("threads = %d, want 1", len(files[0].Threads))
	}
	if files[0].Threads[0].ID != thread.ID {
		t.Errorf("thread ID = %q, want %q", files[0].Threads[0].ID, thread.ID)
	}
}

// E-PENPAL-MCP-TOOLS: verifies penpal_files_in_review identifies oldest pending human thread.
func TestFilesInReview_OldestPending(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	// Create two threads with human comments — oldest should be selected
	thread1 := createTestThread(t, env, "thoughts/pending.md", "First comment")
	time.Sleep(10 * time.Millisecond) // ensure different timestamps
	createTestThread(t, env, "thoughts/pending.md", "Second comment")

	text := callTool(t, env, "penpal_files_in_review", map[string]any{
		"project": env.projName,
	})

	var files []fileWithThreadsResponse
	if err := json.Unmarshal([]byte(text), &files); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(files) != 1 {
		t.Fatalf("files = %d, want 1", len(files))
	}
	if files[0].OldestPending == nil {
		t.Fatal("oldestPending is nil")
	}
	if files[0].OldestPending.ID != thread1.ID {
		t.Errorf("oldestPending ID = %q, want %q (oldest)", files[0].OldestPending.ID, thread1.ID)
	}
}

// E-PENPAL-MCP-WORKING, E-PENPAL-WORKING: verifies files_in_review sets working indicator for pending threads.
func TestFilesInReview_SetsWorkingIndicator(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	thread := createTestThread(t, env, "thoughts/working.md", "Review this")

	// Before calling files_in_review, no working indicator
	if env.store.IsWorking(env.projName, "thoughts/working.md", thread.ID) {
		t.Fatal("expected no working indicator before files_in_review")
	}

	callTool(t, env, "penpal_files_in_review", map[string]any{
		"project": env.projName,
	})

	// After calling files_in_review, working indicator should be set
	if !env.store.IsWorking(env.projName, "thoughts/working.md", thread.ID) {
		t.Error("expected working indicator to be set after files_in_review")
	}
}

// E-PENPAL-MCP-WORKING, E-PENPAL-WORKING: verifies reply clears working indicator for the thread.
func TestReply_ClearsWorkingIndicator(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	thread := createTestThread(t, env, "thoughts/clear-working.md", "Review")

	// Set working indicator via files_in_review
	callTool(t, env, "penpal_files_in_review", map[string]any{
		"project": env.projName,
	})
	if !env.store.IsWorking(env.projName, "thoughts/clear-working.md", thread.ID) {
		t.Fatal("expected working indicator to be set")
	}

	// Reply should clear it
	callTool(t, env, "penpal_reply", map[string]any{
		"project":  env.projName,
		"path":     "thoughts/clear-working.md",
		"threadId": thread.ID,
		"body":     "Done",
	})

	if env.store.IsWorking(env.projName, "thoughts/clear-working.md", thread.ID) {
		t.Error("expected working indicator to be cleared after reply")
	}
}

// E-PENPAL-MCP-WORKING: verifies no oldest pending when agent already replied.
func TestFilesInReview_NoOldestPendingWhenAgentReplied(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	// Create a thread and have the agent reply — no pending thread
	thread := createTestThread(t, env, "thoughts/replied.md", "Please check")
	agentComment := comments.Comment{Author: "claude", Role: "agent", Body: "Done"}
	env.store.AddComment(env.projName, "thoughts/replied.md", thread.ID, agentComment)

	text := callTool(t, env, "penpal_files_in_review", map[string]any{
		"project": env.projName,
	})

	var files []fileWithThreadsResponse
	if err := json.Unmarshal([]byte(text), &files); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(files) != 1 {
		t.Fatalf("files = %d, want 1", len(files))
	}
	if files[0].OldestPending != nil {
		t.Error("expected no oldestPending when agent already replied")
	}
}

// E-PENPAL-CHANGE-SEQ: verifies penpal_wait_for_changes wakes on NotifyChange.
func TestWaitForChanges_Triggered(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	go func() {
		time.Sleep(200 * time.Millisecond)
		env.store.NotifyChange()
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	res, err := env.session.CallTool(ctx, &mcp.CallToolParams{
		Name: "penpal_wait_for_changes",
		Arguments: map[string]any{
			"project": env.projName,
		},
	})
	if err != nil {
		t.Fatalf("CallTool: %v", err)
	}

	tc, ok := res.Content[0].(*mcp.TextContent)
	if !ok {
		t.Fatalf("expected TextContent, got %T", res.Content[0])
	}

	var result map[string]any
	if err := json.Unmarshal([]byte(tc.Text), &result); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if result["changed"] != true {
		t.Errorf("changed = %v, want true", result["changed"])
	}
}
