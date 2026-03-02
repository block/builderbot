package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/cache"
)

func TestAPIProjects_ListsProjects(t *testing.T) {
	s, c, _ := testServer(t)

	seedProject(c, "proj-a", t.TempDir(), nil)
	seedProject(c, "proj-b", t.TempDir(), nil)

	req := httptest.NewRequest(http.MethodGet, "/api/projects", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var projects []APIProject
	if err := json.Unmarshal(rec.Body.Bytes(), &projects); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if len(projects) < 2 {
		t.Fatalf("expected at least 2 projects, got %d", len(projects))
	}

	names := map[string]bool{}
	for _, p := range projects {
		names[p.Name] = true
	}
	if !names["proj-a"] || !names["proj-b"] {
		t.Errorf("expected both proj-a and proj-b, got %v", names)
	}
}

func TestAPIProjects_AddAndCloseStandalone(t *testing.T) {
	s, _, _ := testServer(t)
	dir := t.TempDir()

	// Add
	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/projects", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("add: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// Close
	body, _ = json.Marshal(map[string]string{"path": dir})
	req = httptest.NewRequest(http.MethodDelete, "/api/projects", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("close: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}
}

func TestAPIProjectFiles_NotFound(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodGet, "/api/project/unknown/project", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var groups []APIFileGroupView
	if err := json.Unmarshal(rec.Body.Bytes(), &groups); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if len(groups) != 0 {
		t.Errorf("expected empty array, got %d groups", len(groups))
	}
}

func TestAPIProjectFiles_NoFilesReturnsEmptyArray(t *testing.T) {
	// Regression: Go nil slices encode as JSON null. When a project has no
	// files, the response must be [] not null, or the frontend crashes.
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "empty-proj", dir, nil)

	req := httptest.NewRequest(http.MethodGet, "/api/project/empty-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var groups []APIFileGroupView
	if err := json.Unmarshal(rec.Body.Bytes(), &groups); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if groups == nil {
		t.Error("expected [] but got null: nil slice would crash frontend accessing groups.length")
	}
}

func TestAPIProjectInfo_ReturnsInfo(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, []cache.FileInfo{
		{Project: "test-proj", Name: "plan.md", FullPath: "thoughts/plan.md", Source: "thoughts", ModTime: time.Now()},
	})

	req := httptest.NewRequest(http.MethodGet, "/api/project-info?name=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var info ProjectInfo
	if err := json.Unmarshal(rec.Body.Bytes(), &info); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if info.FileCount != 1 {
		t.Errorf("expected fileCount=1, got %d", info.FileCount)
	}
}

func TestAPIAgents_StatusNoAgent(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodGet, "/api/agents?project=nonexistent", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var resp map[string]interface{}
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if resp["running"] != false {
		t.Errorf("expected running=false, got %v", resp["running"])
	}
}

func TestAPIAgents_MissingProject(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodGet, "/api/agents", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", rec.Code)
	}
}

func TestAPIRawFile_ReturnsContent(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "notes.md"), []byte("# Hello World\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", dir, nil)

	req := httptest.NewRequest(http.MethodGet, "/api/raw?project=test-proj&path=notes.md", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}
	if got := rec.Body.String(); got != "# Hello World\n" {
		t.Errorf("expected file content, got %q", got)
	}
}

func TestAPIInReview_ReturnsGroups(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Create a thread to put a file in review
	createBody, _ := json.Marshal(map[string]interface{}{
		"project": "test-proj",
		"path":    "thoughts/plan.md",
		"anchor":  map[string]string{"selectedText": "text"},
		"author":  "user",
		"role":    "human",
		"body":    "Review",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/threads", bytes.NewReader(createBody))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	req = httptest.NewRequest(http.MethodGet, "/api/in-review", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}

	var groups []ReviewGroup
	if err := json.Unmarshal(rec.Body.Bytes(), &groups); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if len(groups) == 0 {
		t.Error("expected at least 1 review group")
	}
}
