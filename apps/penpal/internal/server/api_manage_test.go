package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/config"
)

// E-PENPAL-REMOVE-WORKSPACE: verifies add and remove workspace round-trip.
func TestAPIWorkspaces_AddAndRemove(t *testing.T) {
	s, _, _ := testServer(t)
	dir := t.TempDir()

	// Add
	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/workspaces", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("add: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// Remove
	body, _ = json.Marshal(map[string]string{"path": dir})
	req = httptest.NewRequest(http.MethodDelete, "/api/workspaces", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("remove: expected 204, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-ADD-SOURCE: verifies POST /api/sources adds a directory tree source.
func TestAPISources_AddTreeSource(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "docs"), 0o755); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", dir, nil)

	body, _ := json.Marshal(map[string]string{
		"project": "test-proj",
		"path":    "docs",
		"name":    "docs",
	})
	req := httptest.NewRequest(http.MethodPost, "/api/sources", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", rec.Code, rec.Body.String())
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/open resolves existing project.
func TestAPIOpen_ExistingProject(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if resp["url"] != "/project/test-proj" {
		t.Errorf("expected url '/project/test-proj', got %q", resp["url"])
	}
}

// E-PENPAL-SSE: verifies /api/navigate returns empty when no pending navigation.
func TestAPINavigate_EmptyByDefault(t *testing.T) {
	s, _, _ := testServer(t)

	req := httptest.NewRequest(http.MethodGet, "/api/navigate", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}
	var resp map[string]string
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["url"] != "" {
		t.Errorf("expected empty url, got %q", resp["url"])
	}
}

// E-PENPAL-SSE: verifies /api/open sets pendingNav consumed by /api/navigate.
func TestAPINavigate_SetByOpen(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	seedProject(c, "test-proj", dir, nil)

	// Call /api/open to set pending navigation
	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("open: expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	// /api/navigate should return the pending URL
	req = httptest.NewRequest(http.MethodGet, "/api/navigate", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	var resp map[string]string
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["url"] != "/project/test-proj" {
		t.Errorf("expected '/project/test-proj', got %q", resp["url"])
	}

	// Second call should return empty (consumed)
	req = httptest.NewRequest(http.MethodGet, "/api/navigate", nil)
	rec = httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	resp = map[string]string{}
	json.Unmarshal(rec.Body.Bytes(), &resp)
	if resp["url"] != "" {
		t.Errorf("expected empty after consume, got %q", resp["url"])
	}
}

// E-PENPAL-API-ROUTES: verifies POST /api/open adds new standalone project.
func TestAPIOpen_NewDirectory(t *testing.T) {
	s, _, _ := testServer(t)
	dir := t.TempDir()

	body, _ := json.Marshal(map[string]string{"path": dir})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	if resp["url"] == "" {
		t.Error("expected non-empty url")
	}
}

// E-PENPAL-DELETE-FILE: verifies POST /api/delete-file removes file from disk.
func TestAPIDeleteFile_Success(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	filePath := filepath.Join(dir, "todelete.md")
	if err := os.WriteFile(filePath, []byte("delete me"), 0o644); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", dir, nil)

	req := httptest.NewRequest(http.MethodPost, "/api/delete-file?project=test-proj&path=todelete.md", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	if _, err := os.Stat(filePath); !os.IsNotExist(err) {
		t.Error("expected file to be deleted")
	}
}

// E-PENPAL-DELETE-PROJECT: verifies POST /api/delete-project removes directory via os.RemoveAll.
func TestAPIDeleteProject_Success(t *testing.T) {
	s, c, _ := testServer(t)

	dir := t.TempDir()
	projDir := filepath.Join(dir, "myproject")
	if err := os.MkdirAll(projDir, 0o755); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", projDir, nil)
	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{Path: projDir})

	req := httptest.NewRequest(http.MethodPost, "/api/delete-project?name=test-proj", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	if _, err := os.Stat(projDir); !os.IsNotExist(err) {
		t.Error("expected project directory to be deleted")
	}
}

// E-PENPAL-API-ROUTES: verifies /api/open uses longest-prefix matching for sub-project.
func TestAPIOpen_PrefersSubProjectOverRoot(t *testing.T) {
	s, c, _ := testServer(t)

	// Create a workspace directory structure with (root) and a sub-project
	wsDir := t.TempDir()
	subDir := filepath.Join(wsDir, "subproj")
	if err := os.MkdirAll(subDir, 0o755); err != nil {
		t.Fatal(err)
	}

	// Create a .md file under the sub-project
	filePath := filepath.Join(subDir, "doc.md")
	if err := os.WriteFile(filePath, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Seed both (root) (workspace root) and subproj (more specific path).
	// (root) is seeded first to simulate the alphabetical ordering bug.
	seedProject(c, "ws/(root)", wsDir, nil)
	seedProject(c, "ws/subproj", subDir, []cache.FileInfo{{FullPath: "doc.md"}})

	body, _ := json.Marshal(map[string]string{"path": filePath})
	req := httptest.NewRequest(http.MethodPost, "/api/open", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", rec.Code, rec.Body.String())
	}

	var resp map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("parse JSON: %v", err)
	}
	// Should route to the sub-project, not (root)
	expected := "/file/ws/subproj/doc.md"
	if resp["url"] != expected {
		t.Errorf("expected url %q, got %q", expected, resp["url"])
	}
}

// E-PENPAL-DELETE-FILE: verifies sidecar cleanup and removeEmptyParents after file deletion.
func TestAPIDeleteFile_CleansUpCommentSidecar(t *testing.T) {
	s, c, cs := testServer(t)

	dir := t.TempDir()
	filePath := filepath.Join(dir, "reviewed.md")
	if err := os.WriteFile(filePath, []byte("content"), 0o644); err != nil {
		t.Fatal(err)
	}
	seedProject(c, "test-proj", dir, nil)

	// Create a comment thread on the file
	_, err := cs.CreateThread("test-proj", "reviewed.md", comments.Anchor{
		SelectedText: "content",
	}, comments.Comment{
		Author: "alice",
		Role:   "human",
		Body:   "Needs work",
	})
	if err != nil {
		t.Fatalf("CreateThread: %v", err)
	}

	// Verify the sidecar exists
	sidecarPath := filepath.Join(dir, ".penpal", "comments", "reviewed.md.json")
	if _, err := os.Stat(sidecarPath); err != nil {
		t.Fatalf("expected sidecar to exist: %v", err)
	}

	// Delete the file via API
	req := httptest.NewRequest(http.MethodPost, "/api/delete-file?project=test-proj&path=reviewed.md", nil)
	rec := httptest.NewRecorder()
	s.ServeHTTP(rec, req)

	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d: %s", rec.Code, rec.Body.String())
	}

	// The .md file should be gone
	if _, err := os.Stat(filePath); !os.IsNotExist(err) {
		t.Error("expected file to be deleted")
	}

	// The sidecar should also be gone
	if _, err := os.Stat(sidecarPath); !os.IsNotExist(err) {
		t.Error("expected comment sidecar to be deleted")
	}

	// ListFilesInReview should return empty
	files, err := cs.ListFilesInReview("test-proj")
	if err != nil {
		t.Fatalf("ListFilesInReview: %v", err)
	}
	if len(files) != 0 {
		t.Errorf("expected 0 files in review, got %d", len(files))
	}
}
