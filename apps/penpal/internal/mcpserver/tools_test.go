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
	anchor := comments.Anchor{SelectedText: "some text"}
	comment := comments.Comment{Author: "human", Role: "human", Body: body}
	thread, err := env.store.CreateThread(env.projName, filePath, anchor, comment)
	if err != nil {
		t.Fatalf("creating thread: %v", err)
	}
	return thread
}

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

	var files []comments.FileInReview
	if err := json.Unmarshal([]byte(text), &files); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(files) != 2 {
		t.Fatalf("files = %d, want 2", len(files))
	}
}

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
