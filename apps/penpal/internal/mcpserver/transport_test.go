package mcpserver

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/comments"
)

// TestMCPOverHTTP_FindProject verifies the full HTTP roundtrip for find_project.
// E-PENPAL-MCP-TRANSPORT: verifies Streamable HTTP transport roundtrip for penpal_find_project.
func TestMCPOverHTTP_FindProject(t *testing.T) {
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
	if result["path"] != env.projDir {
		t.Errorf("path = %q, want %q", result["path"], env.projDir)
	}
}

// TestMCPOverHTTP_ThreadLifecycle exercises create -> list -> reply -> resolve
// through the HTTP transport as a single integration flow.
// E-PENPAL-MCP-TRANSPORT: verifies full thread lifecycle over Streamable HTTP transport.
// E-PENPAL-MCP-TOOLS: exercises create_thread, list_threads, reply, and read_thread in sequence.
func TestMCPOverHTTP_ThreadLifecycle(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	// Create a real file for the create_thread tool
	mdPath := filepath.Join(env.projDir, "thoughts", "lifecycle.md")
	os.MkdirAll(filepath.Dir(mdPath), 0755)
	os.WriteFile(mdPath, []byte("# Lifecycle Test\n\nSome important text here."), 0644)

	// 1. Create thread
	text := callTool(t, env, "penpal_create_thread", map[string]any{
		"project":      env.projName,
		"path":         "thoughts/lifecycle.md",
		"selectedText": "important text",
		"body":         "Please review this section.",
	})

	var thread comments.Thread
	if err := json.Unmarshal([]byte(text), &thread); err != nil {
		t.Fatalf("create unmarshal: %v", err)
	}
	if thread.ID == "" {
		t.Fatal("expected thread ID")
	}

	// 2. List threads — should find the one we created
	text = callTool(t, env, "penpal_list_threads", map[string]any{
		"project": env.projName,
		"path":    "thoughts/lifecycle.md",
	})

	var threads []comments.Thread
	if err := json.Unmarshal([]byte(text), &threads); err != nil {
		t.Fatalf("list unmarshal: %v", err)
	}
	if len(threads) != 1 {
		t.Fatalf("threads = %d, want 1", len(threads))
	}

	// 3. Reply to the thread
	text = callTool(t, env, "penpal_reply", map[string]any{
		"project":  env.projName,
		"path":     "thoughts/lifecycle.md",
		"threadId": thread.ID,
		"body":     "Looks good to me.",
	})

	var replied comments.Thread
	if err := json.Unmarshal([]byte(text), &replied); err != nil {
		t.Fatalf("reply unmarshal: %v", err)
	}
	if len(replied.Comments) != 2 {
		t.Fatalf("comments = %d, want 2", len(replied.Comments))
	}

	// 4. Read the thread back
	text = callTool(t, env, "penpal_read_thread", map[string]any{
		"project":  env.projName,
		"path":     "thoughts/lifecycle.md",
		"threadId": thread.ID,
	})

	var read comments.Thread
	if err := json.Unmarshal([]byte(text), &read); err != nil {
		t.Fatalf("read unmarshal: %v", err)
	}
	if len(read.Comments) != 2 {
		t.Errorf("read comments = %d, want 2", len(read.Comments))
	}
}

// TestMCPOverHTTP_FilesInReview verifies files_in_review through HTTP.
// E-PENPAL-MCP-TRANSPORT: verifies penpal_files_in_review over Streamable HTTP transport.
// E-PENPAL-MCP-TOOLS: verifies files_in_review returns correct file count via HTTP.
func TestMCPOverHTTP_FilesInReview(t *testing.T) {
	env, cleanup := setup(t)
	defer cleanup()

	// Create source files so ListFilesInReview finds them
	for _, name := range []string{"thoughts/a.md", "thoughts/b.md"} {
		p := filepath.Join(env.projDir, name)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte("content"), 0o644); err != nil {
			t.Fatal(err)
		}
	}

	// Create threads on two files
	createTestThread(t, env, "thoughts/a.md", "Comment A")
	createTestThread(t, env, "thoughts/b.md", "Comment B")

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
