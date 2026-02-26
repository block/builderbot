package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/config"
)

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
