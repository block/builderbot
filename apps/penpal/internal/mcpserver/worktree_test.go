package mcpserver

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/discovery"
)

// setupWithWorktree creates a test env with a project that has a worktree.
func setupWithWorktree(t *testing.T) (*testEnv, string, func()) {
	t.Helper()

	env, cleanup := setup(t)

	// Create a worktree directory inside the project
	wtDir := filepath.Join(env.projDir, ".claude", "worktrees", "test-wt")
	if err := os.MkdirAll(wtDir, 0755); err != nil {
		cleanup()
		t.Fatalf("creating worktree dir: %v", err)
	}

	// Create thoughts dir in worktree
	if err := os.MkdirAll(filepath.Join(wtDir, "thoughts"), 0755); err != nil {
		cleanup()
		t.Fatalf("creating worktree thoughts dir: %v", err)
	}

	// Update the project to include worktrees
	env.cache.SetProjects([]discovery.Project{{
		Name:   env.projName,
		Path:   env.projDir,
		Origin: "standalone",
		Worktrees: []discovery.Worktree{
			{Name: filepath.Base(env.projDir), Path: env.projDir, Branch: "main", IsMain: true},
			{Name: "test-wt", Path: wtDir, Branch: "feature-branch"},
		},
	}})

	return env, wtDir, cleanup
}

func TestFindProject_WithWorktree(t *testing.T) {
	env, wtDir, cleanup := setupWithWorktree(t)
	defer cleanup()

	text := callTool(t, env, "penpal_find_project", map[string]any{
		"directory": wtDir,
	})

	var result map[string]string
	if err := json.Unmarshal([]byte(text), &result); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if result["project"] != env.projName {
		t.Errorf("project = %q, want %q", result["project"], env.projName)
	}
	if result["worktree"] != "test-wt" {
		t.Errorf("worktree = %q, want %q", result["worktree"], "test-wt")
	}
}

func TestFindProject_MainWorktree(t *testing.T) {
	env, _, cleanup := setupWithWorktree(t)
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
	if _, ok := result["worktree"]; ok {
		t.Errorf("worktree should not be present for main project, got %q", result["worktree"])
	}
}

func TestCreateThread_InWorktree(t *testing.T) {
	env, wtDir, cleanup := setupWithWorktree(t)
	defer cleanup()

	// Write a file in the worktree
	mdPath := filepath.Join(wtDir, "thoughts", "wt-test.md")
	os.WriteFile(mdPath, []byte("# Worktree Test\n\nThis is worktree content."), 0644)

	text := callTool(t, env, "penpal_create_thread", map[string]any{
		"project":      env.projName,
		"path":         "thoughts/wt-test.md",
		"selectedText": "worktree content",
		"body":         "Comment in worktree.",
		"worktree":     "test-wt",
	})

	var thread comments.Thread
	if err := json.Unmarshal([]byte(text), &thread); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if thread.ID == "" {
		t.Error("thread ID is empty")
	}

	// Verify the sidecar was created in the worktree directory, not the main project
	wtSidecar := filepath.Join(wtDir, ".penpal", "comments", "thoughts", "wt-test.md.json")
	if _, err := os.Stat(wtSidecar); os.IsNotExist(err) {
		t.Error("sidecar file should exist in worktree directory")
	}

	mainSidecar := filepath.Join(env.projDir, ".penpal", "comments", "thoughts", "wt-test.md.json")
	if _, err := os.Stat(mainSidecar); !os.IsNotExist(err) {
		t.Error("sidecar file should NOT exist in main project directory")
	}
}

func TestListThreads_WorktreeScoped(t *testing.T) {
	env, wtDir, cleanup := setupWithWorktree(t)
	defer cleanup()

	// Create a thread in the worktree
	srcPath := filepath.Join(wtDir, "thoughts", "scoped.md")
	os.MkdirAll(filepath.Dir(srcPath), 0755)
	os.WriteFile(srcPath, []byte("test content"), 0644)

	anchor := comments.Anchor{SelectedText: "test"}
	comment := comments.Comment{Author: "human", Role: "human", Body: "worktree comment"}
	env.store.CreateThreadForWorktree(env.projName, "thoughts/scoped.md", "test-wt", anchor, comment)

	// Also create a thread in the main project on the same file path
	mainSrcPath := filepath.Join(env.projDir, "thoughts", "scoped.md")
	os.MkdirAll(filepath.Dir(mainSrcPath), 0755)
	os.WriteFile(mainSrcPath, []byte("main content"), 0644)
	env.store.CreateThread(env.projName, "thoughts/scoped.md", anchor, comments.Comment{Author: "human", Role: "human", Body: "main comment"})

	// List threads for worktree — should only see worktree thread
	text := callTool(t, env, "penpal_list_threads", map[string]any{
		"project":  env.projName,
		"path":     "thoughts/scoped.md",
		"worktree": "test-wt",
	})

	var threads []comments.Thread
	if err := json.Unmarshal([]byte(text), &threads); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(threads) != 1 {
		t.Fatalf("expected 1 worktree thread, got %d", len(threads))
	}
	if threads[0].Comments[0].Body != "worktree comment" {
		t.Errorf("expected worktree comment, got %q", threads[0].Comments[0].Body)
	}

	// List threads without worktree — should see main thread
	text2 := callTool(t, env, "penpal_list_threads", map[string]any{
		"project": env.projName,
		"path":    "thoughts/scoped.md",
	})

	var mainThreads []comments.Thread
	if err := json.Unmarshal([]byte(text2), &mainThreads); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(mainThreads) != 1 {
		t.Fatalf("expected 1 main thread, got %d", len(mainThreads))
	}
	if mainThreads[0].Comments[0].Body != "main comment" {
		t.Errorf("expected main comment, got %q", mainThreads[0].Comments[0].Body)
	}
}

func TestReply_InWorktree(t *testing.T) {
	env, wtDir, cleanup := setupWithWorktree(t)
	defer cleanup()

	// Create a thread in the worktree
	srcPath := filepath.Join(wtDir, "thoughts", "reply-wt.md")
	os.MkdirAll(filepath.Dir(srcPath), 0755)
	os.WriteFile(srcPath, []byte("test"), 0644)

	anchor := comments.Anchor{SelectedText: "test"}
	comment := comments.Comment{Author: "human", Role: "human", Body: "Original"}
	thread, _ := env.store.CreateThreadForWorktree(env.projName, "thoughts/reply-wt.md", "test-wt", anchor, comment)

	text := callTool(t, env, "penpal_reply", map[string]any{
		"project":  env.projName,
		"path":     "thoughts/reply-wt.md",
		"threadId": thread.ID,
		"body":     "Reply in worktree",
		"worktree": "test-wt",
	})

	var result comments.Thread
	if err := json.Unmarshal([]byte(text), &result); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(result.Comments) != 2 {
		t.Fatalf("expected 2 comments, got %d", len(result.Comments))
	}
	if result.Comments[1].Body != "Reply in worktree" {
		t.Errorf("reply body = %q, want %q", result.Comments[1].Body, "Reply in worktree")
	}
}

func TestFilesInReview_WorktreeScoped(t *testing.T) {
	env, wtDir, cleanup := setupWithWorktree(t)
	defer cleanup()

	// Create files and threads in worktree
	srcPath := filepath.Join(wtDir, "thoughts", "review-wt.md")
	os.MkdirAll(filepath.Dir(srcPath), 0755)
	os.WriteFile(srcPath, []byte("content"), 0644)

	anchor := comments.Anchor{SelectedText: "content"}
	comment := comments.Comment{Author: "human", Role: "human", Body: "Review this"}
	env.store.CreateThreadForWorktree(env.projName, "thoughts/review-wt.md", "test-wt", anchor, comment)

	// Also create a thread in main
	mainSrcPath := filepath.Join(env.projDir, "thoughts", "review-main.md")
	os.MkdirAll(filepath.Dir(mainSrcPath), 0755)
	os.WriteFile(mainSrcPath, []byte("main content"), 0644)
	env.store.CreateThread(env.projName, "thoughts/review-main.md", anchor, comments.Comment{Author: "human", Role: "human", Body: "Main review"})

	// files_in_review for worktree should only show worktree files
	text := callTool(t, env, "penpal_files_in_review", map[string]any{
		"project":  env.projName,
		"worktree": "test-wt",
	})

	var files []fileWithThreadsResponse
	if err := json.Unmarshal([]byte(text), &files); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(files) != 1 {
		t.Fatalf("expected 1 worktree file, got %d", len(files))
	}
	if files[0].FilePath != "thoughts/review-wt.md" {
		t.Errorf("file = %q, want %q", files[0].FilePath, "thoughts/review-wt.md")
	}
}
