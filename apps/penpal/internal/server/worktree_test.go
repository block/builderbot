package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/discovery"
)

func TestAPIProjects_IncludesWorktrees(t *testing.T) {
	s, c, _ := testServer(t)

	// Replace all projects with just our test project
	c.SetProjects([]discovery.Project{{
		Name:          "myrepo",
		Path:          "/tmp/myrepo",
		Origin:        "workspace",
		WorkspaceName: "Dev",
		Worktrees: []discovery.Worktree{
			{Name: "myrepo", Path: "/tmp/myrepo", Branch: "main", IsMain: true},
			{Name: "fancy", Path: "/tmp/myrepo/.claude/worktrees/fancy", Branch: "feat"},
		},
	}})

	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/api/projects", nil)
	s.ServeHTTP(rr, req)

	if rr.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200", rr.Code)
	}

	var result []APIProject
	if err := json.NewDecoder(rr.Body).Decode(&result); err != nil {
		t.Fatalf("decode: %v", err)
	}

	if len(result) != 1 {
		t.Fatalf("expected 1 project, got %d", len(result))
	}

	if len(result[0].Worktrees) != 2 {
		t.Fatalf("expected 2 worktrees, got %d", len(result[0].Worktrees))
	}

	if result[0].Worktrees[0].Name != "myrepo" {
		t.Errorf("worktree[0].Name = %q, want %q", result[0].Worktrees[0].Name, "myrepo")
	}
	if !result[0].Worktrees[0].IsMain {
		t.Error("worktree[0].IsMain should be true")
	}
	if result[0].Worktrees[1].Name != "fancy" {
		t.Errorf("worktree[1].Name = %q, want %q", result[0].Worktrees[1].Name, "fancy")
	}
	if result[0].Worktrees[1].Branch != "feat" {
		t.Errorf("worktree[1].Branch = %q, want %q", result[0].Worktrees[1].Branch, "feat")
	}
}

func TestRawFile_Worktree(t *testing.T) {
	s, c, _ := testServer(t)

	projDir := t.TempDir()
	wtDir := filepath.Join(projDir, ".claude", "worktrees", "test-wt")
	os.MkdirAll(wtDir, 0755)

	// Write different content in main vs worktree
	mainFile := filepath.Join(projDir, "thoughts", "doc.md")
	wtFile := filepath.Join(wtDir, "thoughts", "doc.md")
	os.MkdirAll(filepath.Dir(mainFile), 0755)
	os.MkdirAll(filepath.Dir(wtFile), 0755)
	os.WriteFile(mainFile, []byte("# Main Content"), 0644)
	os.WriteFile(wtFile, []byte("# Worktree Content"), 0644)

	project := seedProject(c, "Dev/myrepo", projDir, []cache.FileInfo{
		{Project: "Dev/myrepo", FullPath: "thoughts/doc.md", Name: "doc.md"},
	})

	// Add worktree to project
	projects := c.Projects()
	for i := range projects {
		if projects[i].QualifiedName() == project.QualifiedName() {
			projects[i].Worktrees = []discovery.Worktree{
				{Name: filepath.Base(projDir), Path: projDir, Branch: "main", IsMain: true},
				{Name: "test-wt", Path: wtDir, Branch: "feat"},
			}
		}
	}
	c.SetProjects(projects)

	// Request without worktree — should get main content
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/api/raw?project=Dev/myrepo&path=thoughts/doc.md", nil)
	s.ServeHTTP(rr, req)

	if rr.Code != http.StatusOK {
		t.Fatalf("main status = %d, want 200", rr.Code)
	}
	if body := rr.Body.String(); body != "# Main Content" {
		t.Errorf("main content = %q, want %q", body, "# Main Content")
	}

	// Request with worktree — should get worktree content
	rr2 := httptest.NewRecorder()
	req2 := httptest.NewRequest(http.MethodGet, "/api/raw?project=Dev/myrepo&path=thoughts/doc.md&worktree=test-wt", nil)
	s.ServeHTTP(rr2, req2)

	if rr2.Code != http.StatusOK {
		t.Fatalf("worktree status = %d, want 200", rr2.Code)
	}
	if body := rr2.Body.String(); body != "# Worktree Content" {
		t.Errorf("worktree content = %q, want %q", body, "# Worktree Content")
	}
}

func TestThreads_WorktreeIsolation(t *testing.T) {
	s, c, cs := testServer(t)

	projDir := t.TempDir()
	wtDir := filepath.Join(projDir, ".claude", "worktrees", "test-wt")
	os.MkdirAll(wtDir, 0755)

	// Create the file in both main and worktree
	mainFile := filepath.Join(projDir, "thoughts", "isolated.md")
	wtFile := filepath.Join(wtDir, "thoughts", "isolated.md")
	os.MkdirAll(filepath.Dir(mainFile), 0755)
	os.MkdirAll(filepath.Dir(wtFile), 0755)
	os.WriteFile(mainFile, []byte("main"), 0644)
	os.WriteFile(wtFile, []byte("wt"), 0644)

	project := seedProject(c, "Dev/repo", projDir, nil)

	projects := c.Projects()
	for i := range projects {
		if projects[i].QualifiedName() == project.QualifiedName() {
			projects[i].Worktrees = []discovery.Worktree{
				{Name: filepath.Base(projDir), Path: projDir, Branch: "main", IsMain: true},
				{Name: "test-wt", Path: wtDir, Branch: "feat"},
			}
		}
	}
	c.SetProjects(projects)

	// Create thread in main
	anchor := comments.Anchor{SelectedText: "main"}
	cs.CreateThread("Dev/repo", "thoughts/isolated.md", anchor, comments.Comment{Author: "human", Role: "human", Body: "main comment"})

	// Create thread in worktree
	cs.CreateThreadForWorktree("Dev/repo", "thoughts/isolated.md", "test-wt", anchor, comments.Comment{Author: "human", Role: "human", Body: "wt comment"})

	// GET threads for main
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/api/threads?project=Dev/repo&path=thoughts/isolated.md", nil)
	s.ServeHTTP(rr, req)

	var mainThreads []threadResponse
	json.NewDecoder(rr.Body).Decode(&mainThreads)
	if len(mainThreads) != 1 {
		t.Fatalf("expected 1 main thread, got %d", len(mainThreads))
	}
	if mainThreads[0].Comments[0].Body != "main comment" {
		t.Errorf("main thread body = %q", mainThreads[0].Comments[0].Body)
	}

	// GET threads for worktree
	rr2 := httptest.NewRecorder()
	req2 := httptest.NewRequest(http.MethodGet, "/api/threads?project=Dev/repo&path=thoughts/isolated.md&worktree=test-wt", nil)
	s.ServeHTTP(rr2, req2)

	var wtThreads []threadResponse
	json.NewDecoder(rr2.Body).Decode(&wtThreads)
	if len(wtThreads) != 1 {
		t.Fatalf("expected 1 worktree thread, got %d", len(wtThreads))
	}
	if wtThreads[0].Comments[0].Body != "wt comment" {
		t.Errorf("wt thread body = %q", wtThreads[0].Comments[0].Body)
	}
}

func TestCreateThread_ViaAPI_Worktree(t *testing.T) {
	s, c, _ := testServer(t)

	projDir := t.TempDir()
	wtDir := filepath.Join(projDir, ".claude", "worktrees", "test-wt")
	os.MkdirAll(filepath.Join(wtDir, "thoughts"), 0755)
	os.WriteFile(filepath.Join(wtDir, "thoughts", "api-create.md"), []byte("# Test\n\nSome content here"), 0644)

	project := seedProject(c, "Dev/repo", projDir, nil)
	projects := c.Projects()
	for i := range projects {
		if projects[i].QualifiedName() == project.QualifiedName() {
			projects[i].Worktrees = []discovery.Worktree{
				{Name: filepath.Base(projDir), Path: projDir, Branch: "main", IsMain: true},
				{Name: "test-wt", Path: wtDir, Branch: "feat"},
			}
		}
	}
	c.SetProjects(projects)

	body := `{"project":"Dev/repo","path":"thoughts/api-create.md","anchor":{"selectedText":"Some content"},"author":"human","role":"human","body":"Review this","worktree":"test-wt"}`
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/api/threads", strings.NewReader(body))
	s.ServeHTTP(rr, req)

	if rr.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", rr.Code, rr.Body.String())
	}

	// Verify sidecar is in worktree
	wtSidecar := filepath.Join(wtDir, ".penpal", "comments", "thoughts", "api-create.md.json")
	if _, err := os.Stat(wtSidecar); os.IsNotExist(err) {
		t.Error("sidecar should exist in worktree dir")
	}
	mainSidecar := filepath.Join(projDir, ".penpal", "comments", "thoughts", "api-create.md.json")
	if _, err := os.Stat(mainSidecar); !os.IsNotExist(err) {
		t.Error("sidecar should NOT exist in main dir")
	}
}
